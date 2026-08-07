//! Bidirectional LFM2 backbone + GLiNER layer fuser in native Candle (Task 8).
//!
//! Faithful port of `code_modification/lfm2_bi.py` over the causal
//! `modeling_lfm2.py` backbone:
//! - symmetric (center-padded) depthwise short-conv layers,
//! - fully bidirectional attention (no causal mask; 4D mask with -inf),
//! - GQA with per-head RMSNorm and half-split RoPE,
//! - squeeze-and-excitation `LayersFuser` over all per-layer hidden states.
//!
//! Task 9 extends this module with the loaded runtime: tokenization, the
//! GLiNER head (`rnn`, markerV1 span representations, prompt projection), and
//! the windowed extraction pipeline over the fused encoder output.
//
// The module keeps a module-level lint allowance for the shared Task 8 → Task 9
// contract surface.
#![allow(dead_code)]

use std::sync::Mutex;

use candle_core::{D, Device, IndexOp, Module, Tensor};
use candle_nn::ops::{sigmoid, softmax};
use candle_nn::rnn::Direction;
use candle_nn::{Conv1d, Conv1dConfig, Embedding, LSTM, LSTMConfig, Linear, RNN, VarBuilder};
use tokenizers::{Encoding, Tokenizer};

use crate::models::EntityCandidate;
use crate::service::MemoryError;

use super::config::{LayerKind, Lfm2BiConfig};
use super::decode;
use super::tensors;

/// RMSNorm matching the upstream `Lfm2RMSNorm`: normalize over the last dim
/// with `eps`, scale by the learned weight. F32-only checkpoint.
fn rms_norm(xs: &Tensor, weight: &Tensor, eps: f64) -> candle_core::Result<Tensor> {
    candle_nn::ops::rms_norm(xs, weight, eps as f32)
}

/// Cached `(seq_len, cos, sin)` triple for the RoPE cache.
type CosSinCache = Mutex<Option<(usize, Tensor, Tensor)>>;

/// RoPE cache: `inv_freq` derived from `rope_theta` (computed, never stored in
/// the checkpoint), plus a per-sequence-length cos/sin cache.
#[derive(Debug)]
struct RotaryCache {
    inv_freq: Tensor,
    cache: CosSinCache,
}

impl RotaryCache {
    fn new(head_dim: usize, rope_theta: f64, device: &Device) -> candle_core::Result<Self> {
        let half = head_dim / 2;
        let values: Vec<f32> = (0..half)
            .map(|index| {
                let freq = rope_theta.powf((2 * index) as f64 / head_dim as f64);
                freq.recip() as f32
            })
            .collect();
        let inv_freq = Tensor::from_vec(values, (half,), device)?;
        Ok(Self {
            inv_freq,
            cache: Mutex::new(None),
        })
    }

    /// Returns `(cos, sin)` of shape `[seq_len, head_dim / 2]` — the exact input
    /// contract of `candle_nn::rope`.
    fn cos_sin(&self, seq_len: usize, device: &Device) -> candle_core::Result<(Tensor, Tensor)> {
        if let Some((cached_len, cos, sin)) = self.cache.lock().unwrap().as_ref()
            && *cached_len == seq_len
        {
            return Ok((cos.clone(), sin.clone()));
        }
        let positions =
            Tensor::arange(0u32, seq_len as u32, device)?.to_dtype(candle_core::DType::F32)?;
        let freqs = positions
            .unsqueeze(1)?
            .matmul(&self.inv_freq.unsqueeze(0)?)?;
        let cos = freqs.cos()?;
        let sin = freqs.sin()?;
        *self.cache.lock().unwrap() = Some((seq_len, cos.clone(), sin.clone()));
        Ok((cos, sin))
    }
}

/// GQA expansion: `[B, kv_heads, S, D]` -> `[B, kv_heads * n_rep, S, D]`.
fn repeat_kv(xs: &Tensor, n_rep: usize) -> candle_core::Result<Tensor> {
    if n_rep == 1 {
        return Ok(xs.clone());
    }
    let (batch, kv_heads, seq, head_dim) = xs.dims4()?;
    xs.unsqueeze(2)?
        .expand((batch, kv_heads, n_rep, seq, head_dim))?
        .reshape((batch, kv_heads * n_rep, seq, head_dim))
}

/// Tunes out hidden states of padding tokens before a short-conv layer,
/// mirroring `apply_mask_to_padding_states` (only when batch and seq exceed 1).
fn apply_mask_to_padding_states(
    hidden_states: &Tensor,
    attention_mask: &Tensor,
) -> candle_core::Result<Tensor> {
    let (batch, seq, _) = hidden_states.dims3()?;
    if batch > 1 && seq > 1 {
        hidden_states.broadcast_mul(&attention_mask.unsqueeze(2)?)
    } else {
        Ok(hidden_states.clone())
    }
}

/// Bidirectional 4D attention mask from a 2D `[B, S]` 0/1 mask, mirroring
/// `lfm2_bi.py`: 0 = attend, `f32::MIN` (≈ -inf) = ignore; rows whose query
/// position is padding are fully masked.
fn build_bi_mask(attention_mask: &Tensor) -> candle_core::Result<Tensor> {
    let pair = attention_mask
        .unsqueeze(2)?
        .broadcast_mul(&attention_mask.unsqueeze(1)?)?;
    // (1 - pair) * f32::MIN  ==  pair * f32::MAX + f32::MIN.
    let mask = pair.affine(f32::MAX as f64, f32::MIN as f64)?;
    mask.unsqueeze(1)
}

/// Depthwise short-conv block (`Lfm2ShortConv`, bidirectional padding).
#[derive(Debug)]
struct Lfm2ShortConv {
    in_proj: Linear,
    out_proj: Linear,
    conv: Conv1d,
}

impl Lfm2ShortConv {
    fn load(vb: &VarBuilder, config: &Lfm2BiConfig, layer_idx: usize) -> candle_core::Result<Self> {
        let hidden = config.hidden_size;
        let in_proj = Linear::new(
            vb.get(
                (3 * hidden, hidden),
                &tensors::conv_in_proj_weight(layer_idx),
            )?,
            None,
        );
        let out_proj = Linear::new(
            vb.get((hidden, hidden), &tensors::conv_out_proj_weight(layer_idx))?,
            None,
        );
        let weight = vb.get(
            (hidden, 1, config.conv_l_cache),
            &tensors::conv_conv_weight(layer_idx),
        )?;
        // candle's conv1d indexes `w[k]` against `input[l - padding + k]`, while
        // PyTorch's nn.Conv1d (the checkpoint's training convention) uses
        // `input[l + padding - k]`. Flip the kernel along the length axis so the
        // upstream correlation orientation is reproduced.
        let weight = weight.flip(&[2])?;
        // Depthwise: groups == hidden. Symmetric padding kernel/2 each side.
        let conv = Conv1d::new(
            weight,
            None,
            Conv1dConfig {
                padding: config.conv_l_cache / 2,
                stride: 1,
                dilation: 1,
                groups: hidden,
                cudnn_fwd_algo: None,
            },
        );
        Ok(Self {
            in_proj,
            out_proj,
            conv,
        })
    }

    /// `out_proj(C * conv(B * x))` with `in_proj(x)` chunked into `[B, C, x]`.
    fn forward(&self, xs: &Tensor, attention_mask: Option<&Tensor>) -> candle_core::Result<Tensor> {
        let xs = match attention_mask {
            Some(mask) => apply_mask_to_padding_states(xs, mask)?,
            None => xs.clone(),
        };
        let bcx = self.in_proj.forward(&xs)?.transpose(1, 2)?;
        let chunks = bcx.chunk(3, 1)?;
        let (b, c, x) = (chunks[0].clone(), chunks[1].clone(), chunks[2].clone());
        let bx = (b * x)?;
        let conv_out = self.conv.forward(&bx)?;
        let y = (c * conv_out)?.transpose(1, 2)?.contiguous()?;
        self.out_proj.forward(&y)
    }
}

/// GQA attention (`Lfm2Attention`) with q/k RMSNorm and half-split RoPE.
#[derive(Debug)]
struct Lfm2Attention {
    q_proj: Linear,
    k_proj: Linear,
    v_proj: Linear,
    out_proj: Linear,
    q_layernorm: Tensor,
    k_layernorm: Tensor,
    num_heads: usize,
    kv_heads: usize,
    head_dim: usize,
    scaling: f64,
    norm_eps: f64,
}

impl Lfm2Attention {
    fn load(vb: &VarBuilder, config: &Lfm2BiConfig, layer_idx: usize) -> candle_core::Result<Self> {
        let hidden = config.hidden_size;
        let q_out = config.num_attention_heads * config.head_dim;
        let kv_out = config.num_key_value_heads * config.head_dim;
        let q_proj = Linear::new(
            vb.get((q_out, hidden), &tensors::attn_q_proj_weight(layer_idx))?,
            None,
        );
        let k_proj = Linear::new(
            vb.get((kv_out, hidden), &tensors::attn_k_proj_weight(layer_idx))?,
            None,
        );
        let v_proj = Linear::new(
            vb.get((kv_out, hidden), &tensors::attn_v_proj_weight(layer_idx))?,
            None,
        );
        let out_proj = Linear::new(
            vb.get((hidden, q_out), &tensors::attn_out_proj_weight(layer_idx))?,
            None,
        );
        let q_layernorm = vb.get(
            (config.head_dim,),
            &tensors::attn_q_layernorm_weight(layer_idx),
        )?;
        let k_layernorm = vb.get(
            (config.head_dim,),
            &tensors::attn_k_layernorm_weight(layer_idx),
        )?;
        Ok(Self {
            q_proj,
            k_proj,
            v_proj,
            out_proj,
            q_layernorm,
            k_layernorm,
            num_heads: config.num_attention_heads,
            kv_heads: config.num_key_value_heads,
            head_dim: config.head_dim,
            scaling: (config.head_dim as f64).powf(-0.5),
            norm_eps: config.norm_eps,
        })
    }

    fn forward(
        &self,
        hidden: &Tensor,
        mask4: &Tensor,
        cos: &Tensor,
        sin: &Tensor,
    ) -> candle_core::Result<Tensor> {
        let (batch, seq, _) = hidden.dims3()?;
        let q =
            self.q_proj
                .forward(hidden)?
                .reshape((batch, seq, self.num_heads, self.head_dim))?;
        let q = rms_norm(&q, &self.q_layernorm, self.norm_eps)?.transpose(1, 2)?;
        let k = self
            .k_proj
            .forward(hidden)?
            .reshape((batch, seq, self.kv_heads, self.head_dim))?;
        let k = rms_norm(&k, &self.k_layernorm, self.norm_eps)?.transpose(1, 2)?;
        let v = self
            .v_proj
            .forward(hidden)?
            .reshape((batch, seq, self.kv_heads, self.head_dim))?
            .transpose(1, 2)?;

        let q = candle_nn::rotary_emb::rope(&q.contiguous()?, cos, sin)?;
        let k = candle_nn::rotary_emb::rope(&k.contiguous()?, cos, sin)?;
        let kv_groups = self.num_heads / self.kv_heads;
        let k = repeat_kv(&k, kv_groups)?;
        let v = repeat_kv(&v, kv_groups)?;

        let scores = q
            .matmul(&k.transpose(2, 3)?)?
            .affine(self.scaling, 0.0)?
            .broadcast_add(mask4)?;
        let probs = softmax(&scores, D::Minus1)?;
        let attn = probs.matmul(&v)?.transpose(1, 2)?;
        let attn = attn.reshape((batch, seq, self.num_heads * self.head_dim))?;
        self.out_proj.forward(&attn)
    }
}

/// SwiGLU MLP (`Lfm2MLP`): `w2(silu(w1(x)) * w3(x))`.
#[derive(Debug)]
struct Lfm2Mlp {
    w1: Linear,
    w2: Linear,
    w3: Linear,
}

impl Lfm2Mlp {
    fn load(vb: &VarBuilder, config: &Lfm2BiConfig, layer_idx: usize) -> candle_core::Result<Self> {
        let hidden = config.hidden_size;
        let intermediate = config.intermediate_size;
        let w1 = Linear::new(
            vb.get((intermediate, hidden), &tensors::mlp_w1_weight(layer_idx))?,
            None,
        );
        let w3 = Linear::new(
            vb.get((intermediate, hidden), &tensors::mlp_w3_weight(layer_idx))?,
            None,
        );
        let w2 = Linear::new(
            vb.get((hidden, intermediate), &tensors::mlp_w2_weight(layer_idx))?,
            None,
        );
        Ok(Self { w1, w2, w3 })
    }

    fn forward(&self, xs: &Tensor) -> candle_core::Result<Tensor> {
        let gate = self.w1.forward(xs)?.silu()?;
        let up = self.w3.forward(xs)?;
        self.w2.forward(&(gate * up)?)
    }
}

/// One `Lfm2DecoderLayer`: pre-norm attention-or-conv, residual, pre-norm MLP,
/// residual.
#[derive(Debug)]
struct Lfm2Layer {
    operator_norm: Tensor,
    ffn_norm: Tensor,
    mlp: Lfm2Mlp,
    attn: Option<Lfm2Attention>,
    conv: Option<Lfm2ShortConv>,
    norm_eps: f64,
}

impl Lfm2Layer {
    fn load(vb: &VarBuilder, config: &Lfm2BiConfig, layer_idx: usize) -> candle_core::Result<Self> {
        let operator_norm = vb.get(
            (config.hidden_size,),
            &tensors::operator_norm_weight(layer_idx),
        )?;
        let ffn_norm = vb.get((config.hidden_size,), &tensors::ffn_norm_weight(layer_idx))?;
        let mlp = Lfm2Mlp::load(vb, config, layer_idx)?;
        let (attn, conv) = match config.layer_types[layer_idx] {
            LayerKind::Conv => (None, Some(Lfm2ShortConv::load(vb, config, layer_idx)?)),
            LayerKind::FullAttention => (Some(Lfm2Attention::load(vb, config, layer_idx)?), None),
        };
        Ok(Self {
            operator_norm,
            ffn_norm,
            mlp,
            attn,
            conv,
            norm_eps: config.norm_eps,
        })
    }

    fn forward(
        &self,
        hidden: &Tensor,
        mask4: &Tensor,
        mask2d: &Tensor,
        cos: &Tensor,
        sin: &Tensor,
    ) -> candle_core::Result<Tensor> {
        let residual = hidden.clone();
        let normalized = rms_norm(hidden, &self.operator_norm, self.norm_eps)?;
        let hidden = match (&self.attn, &self.conv) {
            (Some(attn), None) => attn.forward(&normalized, mask4, cos, sin)?,
            (None, Some(conv)) => conv.forward(&normalized, Some(mask2d))?,
            _ => candle_core::bail!("LFM2 layer has neither attention nor conv (bad layer_types)",),
        };
        let hidden = (hidden + residual)?;
        let normalized = rms_norm(&hidden, &self.ffn_norm, self.norm_eps)?;
        let hidden = (hidden + self.mlp.forward(&normalized)?)?;
        Ok(hidden)
    }
}

/// GLiNER `LayersFuser` (squeeze-and-excitation over the per-layer hidden
/// states). The first encoder output (the embedding) is skipped upstream, so
/// `layer_outputs` must hold exactly `num_hidden_layers` entries.
#[derive(Debug)]
struct LayersFuser {
    squeeze: Linear,
    w1: Linear,
    w2: Linear,
    output_projection: Linear,
}

impl LayersFuser {
    fn load(vb: &VarBuilder, config: &Lfm2BiConfig) -> candle_core::Result<Self> {
        let hidden = config.hidden_size;
        let layers = config.num_hidden_layers;
        let half = layers / 2;
        let squeeze = Linear::new(
            vb.get((1, hidden), &tensors::fuser_squeeze_weight())?,
            Some(vb.get((1,), &tensors::fuser_squeeze_bias())?),
        );
        let w1 = Linear::new(
            vb.get((half, layers), &tensors::fuser_w1_weight())?,
            Some(vb.get((half,), &tensors::fuser_w1_bias())?),
        );
        let w2 = Linear::new(
            vb.get((layers, half), &tensors::fuser_w2_weight())?,
            Some(vb.get((layers,), &tensors::fuser_w2_bias())?),
        );
        let output_projection = Linear::new(
            vb.get((hidden, hidden), &tensors::fuser_output_projection_weight())?,
            Some(vb.get((hidden,), &tensors::fuser_output_projection_bias())?),
        );
        Ok(Self {
            squeeze,
            w1,
            w2,
            output_projection,
        })
    }

    /// `output_projection(sum_k sigmoid(W2(relu(W1(mean_l(squeeze(h_k)))))) * h_k)`.
    fn forward(&self, layer_outputs: &[Tensor]) -> candle_core::Result<Tensor> {
        let u = Tensor::stack(layer_outputs, 1)?; // [B, K, L, D]
        let z = self.squeeze.forward(&u)?.squeeze(3)?; // [B, K, L]
        let z = z.mean(2)?; // [B, K]
        let s = self.w1.forward(&z)?.relu()?; // [B, K/2]
        let s = sigmoid(&self.w2.forward(&s)?)?; // [B, K]
        let s = s.unsqueeze(2)?.unsqueeze(3)?; // [B, K, 1, 1]
        let weighted = u.broadcast_mul(&s)?; // [B, K, L, D]
        let summed = weighted.sum(1)?; // [B, L, D]
        self.output_projection.forward(&summed)
    }
}

/// Bidirectional LFM2 model: token embeddings, 16 mixed conv/attention layers,
/// RoPE cache, final RMSNorm, and the optional layer fuser.
#[derive(Debug)]
pub(crate) struct Lfm2BiModel {
    embed_tokens: Embedding,
    layers: Vec<Lfm2Layer>,
    rotary_emb: RotaryCache,
    embedding_norm: Tensor,
    fuser: Option<LayersFuser>,
    config: Lfm2BiConfig,
}

impl Lfm2BiModel {
    /// Loads the model from a root `VarBuilder` over the upstream checkpoint
    /// (keys carry the `bert_layer.model.` / `bert_layer.layers_fuser.`
    /// prefixes). F32 only.
    pub(crate) fn load(vb: VarBuilder, config: &Lfm2BiConfig) -> candle_core::Result<Self> {
        tensors::adapt_weights(&vb, config)?;
        let embed_tokens = Embedding::new(
            vb.get(
                (config.vocab_size, config.hidden_size),
                &tensors::embed_tokens_weight(),
            )?,
            config.hidden_size,
        );
        let mut layers = Vec::with_capacity(config.num_hidden_layers);
        for layer_idx in 0..config.num_hidden_layers {
            layers.push(Lfm2Layer::load(&vb, config, layer_idx)?);
        }
        let rotary_emb = RotaryCache::new(config.head_dim, config.rope_theta, vb.device())?;
        let embedding_norm = vb.get((config.hidden_size,), &tensors::embedding_norm_weight())?;
        let fuser = if config.fuse_layers {
            Some(LayersFuser::load(&vb, config)?)
        } else {
            None
        };
        Ok(Self {
            embed_tokens,
            layers,
            rotary_emb,
            embedding_norm,
            fuser,
            config: config.clone(),
        })
    }

    /// Runs the bidirectional encoder and returns `[batch, seq, hidden]`:
    /// the fused per-layer output when `fuse_layers`, otherwise the last layer
    /// output after the final `embedding_norm`.
    pub(crate) fn forward(
        &self,
        input_ids: &Tensor,
        attention_mask: &Tensor,
        device: &Device,
    ) -> candle_core::Result<Tensor> {
        let inputs_embeds = self.embed_tokens.forward(input_ids)?;
        let (_batch, seq, _hidden) = inputs_embeds.dims3()?;
        let (cos, sin) = self.rotary_emb.cos_sin(seq, device)?;
        let mask4 = build_bi_mask(attention_mask)?;

        let mut hidden = inputs_embeds;
        let mut all_hidden = Vec::with_capacity(self.layers.len() + 1);
        all_hidden.push(hidden.clone());
        for layer in &self.layers {
            hidden = layer.forward(&hidden, &mask4, attention_mask, &cos, &sin)?;
            all_hidden.push(hidden.clone());
        }

        let hidden = rms_norm(&hidden, &self.embedding_norm, self.config.norm_eps)?;
        match &self.fuser {
            Some(fuser) => fuser.forward(&all_hidden[1..]),
            None => Ok(hidden),
        }
    }
}

/// Bidirectional LSTM over word embeddings (upstream `rnn`). Forward and
/// backward passes each use half the hidden size and are concatenated on the
/// last dim — the classic GLiNER `BiLstmLayer` pattern. Candle's LSTM key
/// names (`weight_ih_l0` / `weight_ih_l0_reverse`, ...) match the PyTorch
/// bidirectional state dict exactly, so no key adaptation is needed.
#[derive(Debug)]
pub(crate) struct BiLstmLayer {
    forward: LSTM,
    backward: LSTM,
}

impl BiLstmLayer {
    fn load(vb: VarBuilder, input_dim: usize, hidden_dim: usize) -> candle_core::Result<Self> {
        if hidden_dim == 0 || !hidden_dim.is_multiple_of(2) {
            return Err(candle_core::Error::Msg(
                "VAGO rnn hidden size must be a positive even number".to_string(),
            ));
        }

        let forward = candle_nn::lstm(
            input_dim,
            hidden_dim,
            LSTMConfig {
                direction: Direction::Forward,
                ..Default::default()
            },
            vb.pp("lstm"),
        )?;
        let backward = candle_nn::lstm(
            input_dim,
            hidden_dim,
            LSTMConfig {
                direction: Direction::Backward,
                ..Default::default()
            },
            vb.pp("lstm"),
        )?;

        Ok(Self { forward, backward })
    }

    fn reverse_time_axis(xs: &Tensor) -> candle_core::Result<Tensor> {
        let seq_len = xs.dim(1)?;
        let mut steps = Vec::with_capacity(seq_len);
        for idx in (0..seq_len).rev() {
            steps.push(xs.i((.., idx, ..))?.contiguous()?);
        }

        let refs = steps.iter().collect::<Vec<_>>();
        Tensor::stack(&refs, 1)
    }

    fn forward(&self, xs: &Tensor) -> candle_core::Result<Tensor> {
        let xs = xs.unsqueeze(0)?;

        let forward_states = self.forward.seq(&xs)?;
        let forward_hidden = forward_states
            .into_iter()
            .map(|state| state.h)
            .collect::<Vec<_>>();
        let forward_refs = forward_hidden.iter().collect::<Vec<_>>();
        let forward = Tensor::stack(&forward_refs, 1)?;

        let reversed_xs = Self::reverse_time_axis(&xs)?;
        let backward_states = self.backward.seq(&reversed_xs)?;
        let mut backward_hidden = backward_states
            .into_iter()
            .map(|state| state.h)
            .collect::<Vec<_>>();
        backward_hidden.reverse();
        let backward_refs = backward_hidden.iter().collect::<Vec<_>>();
        let backward = Tensor::stack(&backward_refs, 1)?;

        Tensor::cat(&[&forward, &backward], 2)?.squeeze(0)
    }
}

/// Two-layer MLP matching upstream `create_projection_layer`:
/// `Linear(in, 4*out), ReLU, Dropout, Linear(4*out, out)` — keys `0.*` and
/// `3.*` under the module prefix (dropout is inference-inert).
#[derive(Debug)]
pub(crate) struct FeedForwardProjection {
    input: candle_nn::Linear,
    output: candle_nn::Linear,
}

impl FeedForwardProjection {
    fn load(
        vb: VarBuilder,
        input_dim: usize,
        hidden_dim: usize,
        output_dim: usize,
    ) -> candle_core::Result<Self> {
        let input = candle_nn::linear(input_dim, hidden_dim, vb.pp("0"))?;
        let output = candle_nn::linear(hidden_dim, output_dim, vb.pp("3"))?;
        Ok(Self { input, output })
    }

    fn forward(&self, xs: &Tensor) -> candle_core::Result<Tensor> {
        let hidden = self.input.forward(xs)?.relu()?;
        self.output.forward(&hidden)
    }
}

/// `SpanMarkerV1` head (span_mode = "markerV1").
///
/// For each candidate span the upstream forward concatenates
/// `[project_start(h)[start]; project_end(h)[end]; mean(h)]`, applies ReLU,
/// and projects the triple back to `hidden_size`. The mean is taken over ALL
/// word rows; `project_first` exists in the state dict but is NOT applied by
/// the upstream forward (dead parameter), so it is not loaded here.
#[derive(Debug)]
pub(crate) struct SpanMarkerV1Layer {
    project_start: FeedForwardProjection,
    project_end: FeedForwardProjection,
    out_project: FeedForwardProjection,
}

impl SpanMarkerV1Layer {
    /// Loads from `span_rep_layer.span_rep_layer.*` (double nesting, exactly
    /// like the classic backend). `out_project` consumes the concatenated
    /// `3 * hidden_size` span feature and projects back to `hidden_size`.
    fn load(vb: VarBuilder, hidden_size: usize) -> candle_core::Result<Self> {
        let project_start = FeedForwardProjection::load(
            vb.pp("project_start"),
            hidden_size,
            hidden_size * 4,
            hidden_size,
        )?;
        let project_end = FeedForwardProjection::load(
            vb.pp("project_end"),
            hidden_size,
            hidden_size * 4,
            hidden_size,
        )?;
        let out_project = FeedForwardProjection::load(
            vb.pp("out_project"),
            hidden_size * 3,
            hidden_size * 4,
            hidden_size,
        )?;
        Ok(Self {
            project_start,
            project_end,
            out_project,
        })
    }

    /// markerV1 forward over already-gathered span start/end rows
    /// (`[S, D]` each) plus the full word-hidden tensor for the mean row.
    fn forward(
        &self,
        start_hidden: &Tensor,
        end_hidden: &Tensor,
        text_hidden: &Tensor,
    ) -> candle_core::Result<Tensor> {
        let hidden = start_hidden.dim(1)?;
        let span_count = start_hidden.dim(0)?;
        let average = text_hidden.mean(0)?;
        let first = average.unsqueeze(0)?.expand((span_count, hidden))?;
        let start = self.project_start.forward(start_hidden)?;
        let end = self.project_end.forward(end_hidden)?;
        let combined = Tensor::cat(&[&start, &end, &first], 1)?.relu()?;
        self.out_project.forward(&combined)
    }
}

/// One encoded input window: raw ids, per-token word ids, and the covered
/// `[window_start, window_end)` slice of the split text words.
struct EncodedWindow {
    input_ids: Vec<u32>,
    word_ids: Vec<Option<u32>>,
    window_start: usize,
    window_end: usize,
}

/// The fully loaded SauerkrautLM LFM2 GLiNER runtime: backbone, head, and
/// tokenization state. Construction and inference are CPU-bound and blocking;
/// callers run them on the blocking pool via `LoadedModel`.
pub(crate) struct LoadedLfm2Gliner {
    /// Bidirectional LFM2 backbone with fused-layer output.
    pub(crate) model: Lfm2BiModel,
    /// Parsed configuration (backbone + GLiNER head contract for Task 9).
    pub(crate) config: Lfm2BiConfig,
    /// Device the model tensors live on.
    pub(crate) device: Device,
    /// Checkpoint tokenizer.
    pub(crate) tokenizer: Tokenizer,
    /// Resolved `<<ENT>>` token id (== `class_token_index` for this checkpoint).
    pub(crate) ent_token_id: u32,
    /// Resolved `<<SEP>>` token id.
    pub(crate) sep_token_id: u32,
    /// Configured entity labels.
    pub(crate) labels: Vec<String>,
    /// Confidence threshold for span acceptance.
    pub(crate) threshold: f64,
    /// Bidirectional LSTM over word embeddings.
    pub(crate) rnn: BiLstmLayer,
    /// markerV1 span representation layer.
    pub(crate) span_rep_layer: SpanMarkerV1Layer,
    /// Prompt (label) representation projection.
    pub(crate) prompt_rep_layer: FeedForwardProjection,
    /// Structured logger.
    pub(crate) logger: crate::logging::StdoutLogger,
    /// Configured batch size (recorded; forwards run one window at a time).
    pub(crate) batch_size: usize,
    /// Configured max padded tokens per batch (recorded).
    pub(crate) max_batch_tokens: usize,
}

impl std::fmt::Debug for LoadedLfm2Gliner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedLfm2Gliner")
            .field("config", &self.config)
            .field("device", &self.device)
            .field("labels", &self.labels)
            .field("threshold", &self.threshold)
            .finish()
    }
}

impl LoadedLfm2Gliner {
    /// Builds the full runtime from a root `VarBuilder` over the upstream
    /// checkpoint, resolving the marker token ids from the tokenizer and
    /// validating them against the parsed config.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_from_var_builder(
        vb: VarBuilder,
        tokenizer: Tokenizer,
        config: Lfm2BiConfig,
        labels: Vec<String>,
        threshold: f64,
        device: &Device,
        logger: crate::logging::StdoutLogger,
        batch_size: usize,
        max_batch_tokens: usize,
    ) -> Result<Self, MemoryError> {
        if config.span_mode != "markerV1" {
            return Err(MemoryError::ConfigInvalid(format!(
                "unsupported span_mode `{}`; VAGO backend only supports markerV1",
                config.span_mode
            )));
        }
        if config.subtoken_pooling != "first" {
            return Err(MemoryError::ConfigInvalid(format!(
                "unsupported subtoken_pooling `{}`; VAGO backend only supports first",
                config.subtoken_pooling
            )));
        }

        // Resolve `<<ENT>>`/`<<SEP>>` from the tokenizer, falling back to the
        // config constants, and require `<<ENT>>` to agree with
        // `class_token_index` (prompt extraction matches rows by that id).
        let ent_token_id = tokenizer
            .token_to_id("<<ENT>>")
            .unwrap_or(config.ent_token_id);
        if ent_token_id != config.class_token_index {
            return Err(MemoryError::ConfigInvalid(format!(
                "tokenizer <<ENT>> id {ent_token_id} disagrees with \
                 gliner_config class_token_index {}",
                config.class_token_index
            )));
        }
        let sep_token_id = tokenizer
            .token_to_id("<<SEP>>")
            .unwrap_or(config.sep_token_id);

        let model = Lfm2BiModel::load(vb.clone(), &config)
            .map_err(|err| MemoryError::Storage(format!("failed to build LFM2 backbone: {err}")))?;
        let rnn = BiLstmLayer::load(vb.pp("rnn"), config.hidden_size, config.hidden_size / 2)
            .map_err(|err| MemoryError::Storage(format!("failed to load rnn: {err}")))?;
        let span_rep_layer = SpanMarkerV1Layer::load(
            vb.pp("span_rep_layer").pp("span_rep_layer"),
            config.hidden_size,
        )
        .map_err(|err| MemoryError::Storage(format!("failed to load span_rep_layer: {err}")))?;
        let prompt_rep_layer = FeedForwardProjection::load(
            vb.pp("prompt_rep_layer"),
            config.hidden_size,
            config.hidden_size * 4,
            config.hidden_size,
        )
        .map_err(|err| MemoryError::Storage(format!("failed to load prompt_rep_layer: {err}")))?;

        Ok(Self {
            model,
            config,
            device: device.clone(),
            tokenizer,
            ent_token_id,
            sep_token_id,
            labels,
            threshold,
            rnn,
            span_rep_layer,
            prompt_rep_layer,
            logger,
            batch_size,
            max_batch_tokens,
        })
    }

    /// Encodes the prompt plus the longest text-word prefix that fits into
    /// `config.max_len` tokens, mirroring the classic window encoder.
    fn encode_window(
        &self,
        prompt_words: &[String],
        text_words: &[(String, (usize, usize))],
        window_start: usize,
    ) -> Result<(Encoding, usize), MemoryError> {
        let mut last_fit = None;

        for window_end in window_start + 1..=text_words.len() {
            let mut input_words =
                Vec::with_capacity(prompt_words.len() + window_end - window_start);
            input_words.extend(prompt_words.iter().cloned());
            input_words.extend(
                text_words[window_start..window_end]
                    .iter()
                    .map(|(word, _)| word.clone()),
            );

            let encoding = self
                .tokenizer
                .encode(input_words, true)
                .map_err(|err| MemoryError::Storage(format!("tokenization failed: {err}")))?;

            if encoding.len() > self.config.max_len {
                break;
            }

            last_fit = Some((encoding, window_end));
        }

        last_fit.ok_or_else(|| {
            MemoryError::Storage(format!(
                "VAGO GLiNER input window does not fit into max sequence length {}",
                self.config.max_len
            ))
        })
    }

    /// Single-window forward pass (batch size 1) returning `[seq, hidden]`.
    fn run_forward(&self, input_ids: &[u32]) -> Result<Tensor, MemoryError> {
        // The LFM2 backbone expects an F32 0/1 mask (bi-mask math and padding
        // scaling both broadcast against F32 hidden states).
        let attention_mask = vec![1.0f32; input_ids.len()];

        let input_ids = Tensor::new(input_ids, &self.device)
            .map_err(|err| MemoryError::Storage(format!("tensor error: {err}")))?
            .unsqueeze(0)
            .map_err(|err| MemoryError::Storage(format!("unsqueeze error: {err}")))?;
        let attention_mask = Tensor::new(attention_mask, &self.device)
            .map_err(|err| MemoryError::Storage(format!("mask tensor error: {err}")))?
            .unsqueeze(0)
            .map_err(|err| MemoryError::Storage(format!("mask unsqueeze error: {err}")))?;

        self.model
            .forward(&input_ids, &attention_mask, &self.device)
            .map_err(|err| MemoryError::Storage(format!("forward pass failed: {err}")))?
            .squeeze(0)
            .map_err(|err| MemoryError::Storage(format!("squeeze failed: {err}")))
    }

    /// Prompt marker positions: `input_ids[i] == ent_token_id` with a word id
    /// inside the prompt (`word_id < prompt_word_count`).
    fn collect_prompt_entity_positions(
        &self,
        input_ids: &[u32],
        word_ids: &[Option<u32>],
        prompt_word_count: usize,
    ) -> Vec<usize> {
        decode::collect_ent_token_positions(
            input_ids,
            word_ids,
            self.ent_token_id,
            prompt_word_count,
        )
    }

    /// Gathers first-subtoken hidden rows for every text word, mirroring the
    /// classic `extract_word_representations` (subtoken_pooling = "first").
    fn extract_word_representations(
        &self,
        hidden: &Tensor,
        word_ids: &[Option<u32>],
        prompt_word_count: usize,
        text_offsets: &[(usize, usize)],
    ) -> Result<(Tensor, Vec<(usize, usize)>), MemoryError> {
        let mut prev_word_id = None;
        let mut word_states = Vec::new();
        let mut word_offsets = Vec::new();

        for (token_index, word_id) in word_ids.iter().enumerate() {
            let Some(word_id) = *word_id else {
                prev_word_id = None;
                continue;
            };

            if Some(word_id) == prev_word_id {
                continue;
            }
            prev_word_id = Some(word_id);

            if word_id < prompt_word_count as u32 {
                continue;
            }

            let text_word_index = (word_id as usize).saturating_sub(prompt_word_count);
            if text_word_index >= text_offsets.len() {
                continue;
            }

            let word_hidden = hidden
                .narrow(0, token_index, 1)
                .map_err(|err| MemoryError::Storage(format!("word narrow failed: {err}")))?
                .squeeze(0)
                .map_err(|err| MemoryError::Storage(format!("word squeeze failed: {err}")))?;
            word_states.push(word_hidden);
            word_offsets.push(text_offsets[text_word_index]);
        }

        if word_states.is_empty() {
            return Err(MemoryError::Storage(
                "VAGO tokenization produced no word-level text embeddings".to_string(),
            ));
        }

        let word_state_refs = word_states.iter().collect::<Vec<_>>();
        let word_tensor = Tensor::stack(&word_state_refs, 0)
            .map_err(|err| MemoryError::Storage(format!("word stack failed: {err}")))?;

        Ok((word_tensor, word_offsets))
    }

    /// Projects the hidden rows at the `<<ENT>>` prompt positions into label
    /// representations `[C, D]`.
    fn build_label_representations(
        &self,
        hidden: &Tensor,
        entity_token_positions: &[usize],
    ) -> Result<Tensor, MemoryError> {
        let mut prompt_labels = Vec::with_capacity(entity_token_positions.len());

        for &entity_pos in entity_token_positions {
            let label_hidden = hidden
                .narrow(0, entity_pos, 1)
                .map_err(|err| MemoryError::Storage(format!("label narrow failed: {err}")))?
                .squeeze(0)
                .map_err(|err| MemoryError::Storage(format!("label squeeze failed: {err}")))?;
            prompt_labels.push(label_hidden);
        }

        let prompt_label_refs = prompt_labels.iter().collect::<Vec<_>>();
        let prompt_label_embeddings = Tensor::stack(&prompt_label_refs, 0)
            .map_err(|err| MemoryError::Storage(format!("label stack failed: {err}")))?;

        self.prompt_rep_layer
            .forward(&prompt_label_embeddings)
            .map_err(|err| MemoryError::Storage(format!("prompt projection failed: {err}")))
    }

    /// Enumerates spans over the LSTM-output word embeddings and computes the
    /// markerV1 span representations `[S, D]` plus the matching `(start, end)`
    /// word-index pairs.
    fn compute_span_scores(
        &self,
        text_hidden: &Tensor,
    ) -> Result<(Tensor, Vec<(usize, usize)>), MemoryError> {
        let timer = std::time::Instant::now();
        let text_len = text_hidden
            .dim(0)
            .map_err(|err| MemoryError::Storage(format!("dim error: {err}")))?;
        let span_indices = decode::enumerate_span_indices(text_len, self.config.max_width);
        if span_indices.is_empty() {
            let empty = Tensor::zeros(
                (0, self.config.hidden_size),
                candle_core::DType::F32,
                &self.device,
            )
            .map_err(|err| MemoryError::Storage(format!("empty span tensor: {err}")))?;
            return Ok((empty, span_indices));
        }

        let starts = span_indices
            .iter()
            .map(|(start, _)| *start as u32)
            .collect::<Vec<_>>();
        let ends = span_indices
            .iter()
            .map(|(_, end)| *end as u32)
            .collect::<Vec<_>>();
        let start_indices = Tensor::new(starts.as_slice(), &self.device)
            .map_err(|err| MemoryError::Storage(format!("start index tensor failed: {err}")))?;
        let end_indices = Tensor::new(ends.as_slice(), &self.device)
            .map_err(|err| MemoryError::Storage(format!("end index tensor failed: {err}")))?;
        let start_hidden = text_hidden
            .index_select(&start_indices, 0)
            .map_err(|err| MemoryError::Storage(format!("start gather failed: {err}")))?;
        let end_hidden = text_hidden
            .index_select(&end_indices, 0)
            .map_err(|err| MemoryError::Storage(format!("end gather failed: {err}")))?;
        let span_representations = self
            .span_rep_layer
            .forward(&start_hidden, &end_hidden, text_hidden)
            .map_err(|err| MemoryError::Storage(format!("span projection failed: {err}")))?;

        self.logger.log(
            crate::service::log_event(
                "ner.vago.span_scores.done",
                crate::service::log_args_with_duration(
                    serde_json::json!({"text_words": text_len}),
                    timer.elapsed(),
                ),
                serde_json::json!({"span_count": span_indices.len()}),
                None,
                None,
                None,
            ),
            crate::logging::LogLevel::Debug,
        );

        Ok((span_representations, span_indices))
    }

    /// Scores every enumerated span against the label representations and
    /// appends the thresholded entities to `all_spans`.
    #[allow(clippy::too_many_arguments)]
    fn decode_window(
        &self,
        text: &str,
        text_words: &[(String, (usize, usize))],
        labels: &[String],
        prompt_word_count: usize,
        window: &EncodedWindow,
        hidden: &Tensor,
        all_spans: &mut Vec<decode::ScoredEntity>,
    ) -> Result<(), MemoryError> {
        let entity_token_positions = self.collect_prompt_entity_positions(
            &window.input_ids,
            &window.word_ids,
            prompt_word_count,
        );
        if entity_token_positions.len() != labels.len() {
            return Err(MemoryError::Storage(format!(
                "VAGO prompt extraction mismatch: expected {} entity tokens, found {}",
                labels.len(),
                entity_token_positions.len()
            )));
        }
        let label_representations =
            self.build_label_representations(hidden, &entity_token_positions)?;
        let window_offsets = text_words[window.window_start..window.window_end]
            .iter()
            .map(|(_, offsets)| *offsets)
            .collect::<Vec<_>>();
        let (word_hidden, word_offsets) = self.extract_word_representations(
            hidden,
            &window.word_ids,
            prompt_word_count,
            &window_offsets,
        )?;
        let text_hidden = self
            .rnn
            .forward(&word_hidden)
            .map_err(|err| MemoryError::Storage(format!("rnn forward failed: {err}")))?;
        let (span_representations, span_indices) = self.compute_span_scores(&text_hidden)?;
        let label_transposed = label_representations
            .t()
            .map_err(|err| MemoryError::Storage(format!("label transpose failed: {err}")))?;
        let score_rows = span_representations
            .matmul(&label_transposed)
            .map_err(|err| MemoryError::Storage(format!("span score matmul failed: {err}")))?
            .to_vec2::<f32>()
            .map_err(|err| MemoryError::Storage(format!("span score transfer failed: {err}")))?;

        let spans_data = span_indices
            .into_iter()
            .zip(score_rows)
            .map(|((start, end), scores)| (start, end, scores))
            .collect::<Vec<_>>();
        all_spans.extend(decode::extract_spans(
            text,
            &spans_data,
            &word_offsets,
            labels,
            self.threshold,
        ));
        Ok(())
    }

    /// Runs the windowed extraction pipeline over `text` with the configured
    /// labels and returns deduplicated, sorted entity candidates.
    pub(crate) fn extract_inner_with_labels(
        &self,
        text: &str,
        labels: &[String],
    ) -> Result<Vec<EntityCandidate>, MemoryError> {
        let text_words = decode::split_text_words(text);
        if text_words.is_empty() {
            return Ok(Vec::new());
        }

        let prompt_words = self.build_prompt_words_for_labels(labels);
        let prompt_word_count = prompt_words.len();

        let mut all_spans = Vec::new();
        let mut window_start = 0;
        while window_start < text_words.len() {
            let (encoding, window_end) =
                self.encode_window(&prompt_words, &text_words, window_start)?;
            let window = EncodedWindow {
                input_ids: encoding.get_ids().to_vec(),
                word_ids: encoding.get_word_ids().to_vec(),
                window_start,
                window_end,
            };
            // KISS batching: one window per forward pass (batch size 1). The
            // configured batch limits are validated and recorded but not yet
            // used for packing; the classic `pack_window_batches` is
            // `pub(super)` to the classic backend and cannot be reused.
            let hidden = self.run_forward(&window.input_ids)?;
            self.decode_window(
                text,
                &text_words,
                labels,
                prompt_word_count,
                &window,
                &hidden,
                &mut all_spans,
            )?;
            if window_end >= text_words.len() {
                break;
            }
            window_start = window_end.saturating_sub(1).max(window_start + 1);
        }

        let final_spans = decode::apply_nms(all_spans);
        let mut candidates = final_spans
            .into_iter()
            .map(|span| EntityCandidate {
                entity_type: span.label,
                canonical_name: span.text,
                aliases: Vec::new(),
            })
            .collect::<Vec<_>>();

        candidates.sort_by(|left, right| left.canonical_name.cmp(&right.canonical_name));
        candidates.dedup_by(|left, right| {
            left.canonical_name == right.canonical_name && left.entity_type == right.entity_type
        });

        Ok(candidates)
    }

    /// Runs the pipeline with the configured labels.
    pub(crate) fn extract_inner(&self, text: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        self.extract_inner_with_labels(text, &self.labels)
    }

    /// Builds the marker prompt: `[<<ENT>>, label, <<ENT>>, label, ..., <<SEP>>]`.
    fn build_prompt_words_for_labels(&self, labels: &[String]) -> Vec<String> {
        let ent_token = self
            .tokenizer
            .id_to_token(self.ent_token_id)
            .unwrap_or_else(|| "<<ENT>>".to_string());
        let mut prompt = Vec::with_capacity(labels.len() * 2 + 1);
        for label in labels {
            prompt.push(ent_token.clone());
            prompt.push(label.clone());
        }
        prompt.push("<<SEP>>".to_string());
        prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::{DType, IndexOp};
    use std::collections::HashMap;

    fn tiny_config(fuse_layers: bool) -> Lfm2BiConfig {
        Lfm2BiConfig {
            vocab_size: 32,
            hidden_size: 8,
            intermediate_size: 16,
            num_hidden_layers: 2,
            num_attention_heads: 2,
            num_key_value_heads: 1,
            head_dim: 4,
            norm_eps: 1e-5,
            rope_theta: 10_000.0,
            conv_l_cache: 3,
            layer_types: vec![LayerKind::Conv, LayerKind::FullAttention],
            max_width: 12,
            max_len: 32,
            ent_token_id: 64_402,
            sep_token_id: 64_403,
            class_token_index: 64_402,
            fuse_layers,
            num_post_fusion_layers: 1,
            num_rnn_layers: 1,
            span_mode: "markerV1".to_string(),
            subtoken_pooling: "first".to_string(),
        }
    }

    fn ids(values: &[u32], device: &Device) -> Tensor {
        Tensor::new(values, device).unwrap().unsqueeze(0).unwrap()
    }

    fn ones_mask(seq: usize, device: &Device) -> Tensor {
        Tensor::ones((1, seq), DType::F32, device).unwrap()
    }

    #[test]
    fn forward_returns_expected_shape_and_finite_values() {
        for fuse_layers in [false, true] {
            let config = tiny_config(fuse_layers);
            let weights = tensors::tiny_weights(&config);
            let vb = VarBuilder::from_tensors(weights, DType::F32, &Device::Cpu);
            let model = Lfm2BiModel::load(vb, &config).expect("tiny model loads");
            let device = Device::Cpu;
            let out = model
                .forward(
                    &ids(&[1, 2, 3, 4, 5], &device),
                    &ones_mask(5, &device),
                    &device,
                )
                .unwrap();
            assert_eq!(out.dims(), &[1, 5, 8], "fuse_layers={fuse_layers}");
            for value in out.flatten_all().unwrap().to_vec1::<f32>().unwrap() {
                assert!(
                    value.is_finite(),
                    "non-finite value {value} (fuse_layers={fuse_layers})"
                );
            }
        }
    }

    #[test]
    fn forward_handles_batches_with_padding() {
        let config = tiny_config(false);
        let weights = tensors::tiny_weights(&config);
        let vb = VarBuilder::from_tensors(weights, DType::F32, &Device::Cpu);
        let model = Lfm2BiModel::load(vb, &config).unwrap();
        let device = Device::Cpu;
        let batch_ids = Tensor::new(&[[1u32, 2, 3, 4, 5], [6, 7, 8, 9, 10]], &device).unwrap();
        let batch_mask =
            Tensor::new(&[[1f32, 1., 1., 1., 1.], [1., 1., 1., 0., 0.]], &device).unwrap();
        let out = model.forward(&batch_ids, &batch_mask, &device).unwrap();
        assert_eq!(out.dims(), &[2, 5, 8]);
        for value in out.flatten_all().unwrap().to_vec1::<f32>().unwrap() {
            assert!(value.is_finite(), "non-finite value {value}");
        }
    }

    /// Builds a model whose short-conv block is an exact per-position identity
    /// and whose attention is uniform over all valid keys (zero q/k, identity
    /// v) — so position 0 averages every position's value.
    fn crafted_bidirectional_model() -> Lfm2BiModel {
        let config = tiny_config(false);
        let mut weights = tensors::tiny_weights(&config);
        let device = &Device::Cpu;
        let hidden = config.hidden_size;
        let head_dim = config.head_dim;

        // Layer 0 (conv): [B | C | x] = [ones | ones | identity] and a
        // center-tap depthwise kernel -> the whole block is the identity.
        let ones_rows = Tensor::ones((hidden, hidden), DType::F32, device).unwrap();
        let id_rows = Tensor::eye(hidden, DType::F32, device).unwrap();
        weights.insert(
            tensors::conv_in_proj_weight(0),
            Tensor::cat(&[&ones_rows, &ones_rows, &id_rows], 0).unwrap(),
        );
        let center_tap: Vec<f32> = (0..hidden).flat_map(|_| vec![0.0, 1.0, 0.0]).collect();
        weights.insert(
            tensors::conv_conv_weight(0),
            Tensor::from_vec(center_tap, (hidden, 1, config.conv_l_cache), device).unwrap(),
        );
        weights.insert(
            tensors::conv_out_proj_weight(0),
            Tensor::eye(hidden, DType::F32, device).unwrap(),
        );

        // Layer 1 (attention): zero q/k -> uniform attention; v keeps the first
        // head_dim components so position 0 averages all positions' values.
        let kv_out = config.num_key_value_heads * head_dim;
        weights.insert(
            tensors::attn_q_proj_weight(1),
            Tensor::zeros((hidden, hidden), DType::F32, device).unwrap(),
        );
        weights.insert(
            tensors::attn_k_proj_weight(1),
            Tensor::zeros((kv_out, hidden), DType::F32, device).unwrap(),
        );
        let identity_pad_right = Tensor::cat(
            &[
                Tensor::eye(head_dim, DType::F32, device).unwrap(),
                Tensor::zeros((head_dim, hidden - head_dim), DType::F32, device).unwrap(),
            ],
            1,
        )
        .unwrap();
        weights.insert(tensors::attn_v_proj_weight(1), identity_pad_right.clone());
        let identity_pad_bottom = Tensor::cat(
            &[
                Tensor::cat(
                    &[
                        Tensor::eye(head_dim, DType::F32, device).unwrap(),
                        Tensor::zeros((head_dim, hidden - head_dim), DType::F32, device).unwrap(),
                    ],
                    1,
                )
                .unwrap(),
                Tensor::zeros((hidden - head_dim, hidden), DType::F32, device).unwrap(),
            ],
            0,
        )
        .unwrap();
        weights.insert(tensors::attn_out_proj_weight(1), identity_pad_bottom);

        // Identity-ish FFNs on both layers: ffn(x) = silu(x) * x.
        for layer_idx in 0..2 {
            let w_in = Tensor::cat(
                &[
                    Tensor::eye(hidden, DType::F32, device).unwrap(),
                    Tensor::zeros((hidden, hidden), DType::F32, device).unwrap(),
                ],
                0,
            )
            .unwrap();
            let w_out = Tensor::cat(
                &[
                    Tensor::eye(hidden, DType::F32, device).unwrap(),
                    Tensor::zeros((hidden, hidden), DType::F32, device).unwrap(),
                ],
                1,
            )
            .unwrap();
            weights.insert(tensors::mlp_w1_weight(layer_idx), w_in.clone());
            weights.insert(tensors::mlp_w3_weight(layer_idx), w_in);
            weights.insert(tensors::mlp_w2_weight(layer_idx), w_out);
        }

        let vb = VarBuilder::from_tensors(weights, DType::F32, device);
        Lfm2BiModel::load(vb, &config).unwrap()
    }

    fn position_zero(out: &Tensor) -> Vec<f32> {
        out.i(0).unwrap().i(0).unwrap().to_vec1::<f32>().unwrap()
    }

    #[test]
    fn bidirectional_attention_reaches_later_positions() {
        let model = crafted_bidirectional_model();
        let device = Device::Cpu;
        let ids_a = ids(&[1, 2, 3, 4, 5], &device);
        let ids_b = ids(&[1, 2, 3, 4, 6], &device);
        let ones = ones_mask(5, &device);

        let out_a = model.forward(&ids_a, &ones, &device).unwrap();
        let out_b = model.forward(&ids_b, &ones, &device).unwrap();
        assert_ne!(
            position_zero(&out_a),
            position_zero(&out_b),
            "position 0 must be influenced by the last token (no causal mask)"
        );
    }

    #[test]
    fn attention_mask_blocks_padding_from_being_attended() {
        let model = crafted_bidirectional_model();
        let device = Device::Cpu;
        let ids_a = ids(&[1, 2, 3, 4, 5], &device);
        let ids_b = ids(&[1, 2, 3, 4, 6], &device);
        let masked = Tensor::new(&[1f32, 1., 1., 1., 0.], &device)
            .unwrap()
            .unsqueeze(0)
            .unwrap();

        let out_a = model.forward(&ids_a, &masked, &device).unwrap();
        let out_b = model.forward(&ids_b, &masked, &device).unwrap();
        assert_eq!(
            position_zero(&out_a),
            position_zero(&out_b),
            "masked padding must not affect position 0"
        );
    }

    /// Tiny deterministic wordpiece tokenizer with the marker tokens, two
    /// labels, and the test words. All ids stay below `tiny_config`'s
    /// `vocab_size` (32).
    fn tiny_vago_tokenizer() -> Tokenizer {
        use tokenizers::models::wordpiece::WordPiece;
        let vocab = [
            ("<<ENT>>".to_string(), 0u32),
            ("<<SEP>>".to_string(), 1),
            ("[UNK]".to_string(), 2),
            ("alice".to_string(), 3),
            ("smith".to_string(), 4),
            ("acme".to_string(), 5),
            ("corp".to_string(), 6),
            ("person".to_string(), 7),
            ("company".to_string(), 8),
        ];
        let wordpiece = WordPiece::builder()
            .vocab(vocab)
            .unk_token("[UNK]".to_string())
            .build()
            .expect("wordpiece");
        Tokenizer::new(wordpiece)
    }

    /// Inserts deterministic head weights (rnn + span_rep_layer +
    /// prompt_rep_layer) into the tiny backbone state dict.
    fn insert_head_weights(map: &mut HashMap<String, Tensor>, config: &Lfm2BiConfig) {
        let device = &Device::Cpu;
        let h = config.hidden_size;
        let rnn_hidden = h / 2;

        for suffix in ["", "_reverse"] {
            map.insert(
                format!("rnn.lstm.weight_ih_l0{suffix}"),
                tensors::rand_tensor(&[4 * rnn_hidden, h], 0.5, device),
            );
            map.insert(
                format!("rnn.lstm.weight_hh_l0{suffix}"),
                tensors::rand_tensor(&[4 * rnn_hidden, rnn_hidden], 0.5, device),
            );
            map.insert(
                format!("rnn.lstm.bias_ih_l0{suffix}"),
                tensors::rand_tensor(&[4 * rnn_hidden], 0.5, device),
            );
            map.insert(
                format!("rnn.lstm.bias_hh_l0{suffix}"),
                tensors::rand_tensor(&[4 * rnn_hidden], 0.5, device),
            );
        }

        let project =
            |map: &mut HashMap<String, Tensor>, name: &str, input: usize, output: usize| {
                let hidden = output * 4;
                map.insert(
                    format!("{name}.0.weight"),
                    tensors::rand_tensor(&[hidden, input], 0.5, device),
                );
                map.insert(
                    format!("{name}.0.bias"),
                    tensors::rand_tensor(&[hidden], 0.5, device),
                );
                map.insert(
                    format!("{name}.3.weight"),
                    tensors::rand_tensor(&[output, hidden], 0.5, device),
                );
                map.insert(
                    format!("{name}.3.bias"),
                    tensors::rand_tensor(&[output], 0.5, device),
                );
            };
        project(map, "span_rep_layer.span_rep_layer.project_start", h, h);
        project(map, "span_rep_layer.span_rep_layer.project_end", h, h);
        project(map, "span_rep_layer.span_rep_layer.out_project", 3 * h, h);
        project(map, "prompt_rep_layer", h, h);
    }

    #[test]
    fn extract_inner_runs_end_to_end_on_tiny_model() {
        let device = Device::Cpu;
        let tokenizer = tiny_vago_tokenizer();
        let mut config = tiny_config(false);
        config.class_token_index = tokenizer.token_to_id("<<ENT>>").unwrap();
        config.ent_token_id = tokenizer.token_to_id("<<ENT>>").unwrap();
        config.sep_token_id = tokenizer.token_to_id("<<SEP>>").unwrap();

        let mut weights = tensors::tiny_weights(&config);
        insert_head_weights(&mut weights, &config);
        let vb = VarBuilder::from_tensors(weights, DType::F32, &device);

        let loaded = LoadedLfm2Gliner::new_from_var_builder(
            vb,
            tokenizer,
            config,
            vec!["person".to_string(), "company".to_string()],
            0.0,
            &device,
            crate::logging::StdoutLogger::new("error"),
            1,
            1536,
        )
        .expect("tiny runtime builds");

        let candidates = loaded
            .extract_inner("alice smith")
            .expect("extraction runs");
        assert!(!candidates.is_empty(), "expected entities, got none");
        for candidate in &candidates {
            assert!(
                "alice smith".contains(&candidate.canonical_name),
                "unexpected entity text {:?}",
                candidate.canonical_name
            );
            assert!(
                candidate.entity_type == "person" || candidate.entity_type == "company",
                "unexpected entity type {:?}",
                candidate.entity_type
            );
        }
    }

    #[test]
    fn new_from_var_builder_rejects_ent_token_class_token_mismatch() {
        let device = Device::Cpu;
        let tokenizer = tiny_vago_tokenizer();
        let config = tiny_config(false);
        let weights = tensors::tiny_weights(&config);
        let vb = VarBuilder::from_tensors(weights, DType::F32, &device);

        let error = LoadedLfm2Gliner::new_from_var_builder(
            vb,
            tokenizer,
            config,
            vec!["person".to_string()],
            0.5,
            &device,
            crate::logging::StdoutLogger::new("error"),
            1,
            1536,
        )
        .expect_err("class_token_index mismatch must fail");
        assert!(
            error.to_string().contains("class_token_index"),
            "unexpected error: {error}"
        );
    }
}
