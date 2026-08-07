//! Bidirectional LFM2 backbone + GLiNER layer fuser in native Candle (Task 8).
//!
//! Faithful port of `code_modification/lfm2_bi.py` over the causal
//! `modeling_lfm2.py` backbone:
//! - symmetric (center-padded) depthwise short-conv layers,
//! - fully bidirectional attention (no causal mask; 4D mask with -inf),
//! - GQA with per-head RMSNorm and half-split RoPE,
//! - squeeze-and-excitation `LayersFuser` over all per-layer hidden states.
//!
//! Span decoding and tokenization land in Task 9; this module only produces the
//! fused encoder output `[batch, seq, hidden]`.
//
// Dormant API until Task 9 wires the checkpoint load path (see `config.rs`).
#![allow(dead_code)]

use std::sync::Mutex;

use candle_core::{D, Device, Module, Tensor};
use candle_nn::ops::{sigmoid, softmax};
use candle_nn::{Conv1d, Conv1dConfig, Embedding, Linear, VarBuilder};

use super::config::{LayerKind, Lfm2BiConfig};
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

/// Placeholder for the loaded LFM2 GLiNER runtime: Task 8 wires the backbone
/// load path; Task 9 adds tokenization and span decoding on top.
#[derive(Debug)]
pub(crate) struct LoadedLfm2Gliner {
    /// Bidirectional LFM2 backbone with fused-layer output.
    pub(crate) model: Lfm2BiModel,
    /// Parsed configuration (backbone + GLiNER head contract for Task 9).
    pub(crate) config: Lfm2BiConfig,
    /// Device the model tensors live on.
    pub(crate) device: Device,
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::{DType, IndexOp};

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
            ent_token_id: 128_002,
            sep_token_id: 128_003,
            class_token_index: 64_402,
            fuse_layers,
            num_post_fusion_layers: 1,
            num_rnn_layers: 1,
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
}
