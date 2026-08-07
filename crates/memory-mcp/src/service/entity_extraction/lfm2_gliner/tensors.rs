//! Upstream state-dict name mapping and weight adaptation (Task 8).
//!
//! The GLiNER `Encoder` wraps the bidirectional LFM2 backbone under
//! `bert_layer.model.*` and the layer fuser under `bert_layer.layers_fuser.*`.
//! This module owns every upstream tensor name so the model loader and the
//! checkpoint-adaptation gate stay in one place.
//
// Dormant API until Task 9 wires the checkpoint load path (see `config.rs`).
#![allow(dead_code)]

use candle_core::Result;
use candle_nn::VarBuilder;

#[cfg(test)]
use candle_core::{DType, Device, Tensor};
#[cfg(test)]
use std::collections::HashMap;

use super::config::{LayerKind, Lfm2BiConfig};

/// Prefix of every backbone tensor in the upstream checkpoint
/// (`bert_layer.model.*` — the GLiNER `Encoder.bert_layer` transformer wrapper).
pub(crate) const BACKBONE_PREFIX: &str = "bert_layer.model.";
/// Prefix of every `Encoder`-owned tensor (`bert_layer.*`, including the fuser).
pub(crate) const ENCODER_PREFIX: &str = "bert_layer.";

/// `bert_layer.model.embed_tokens.weight`
pub(crate) fn embed_tokens_weight() -> String {
    format!("{BACKBONE_PREFIX}embed_tokens.weight")
}

/// `bert_layer.model.embedding_norm.weight`
pub(crate) fn embedding_norm_weight() -> String {
    format!("{BACKBONE_PREFIX}embedding_norm.weight")
}

/// `bert_layer.model.layers.{layer}.operator_norm.weight`
pub(crate) fn operator_norm_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.operator_norm.weight")
}

/// `bert_layer.model.layers.{layer}.ffn_norm.weight`
pub(crate) fn ffn_norm_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.ffn_norm.weight")
}

/// `bert_layer.model.layers.{layer}.self_attn.q_proj.weight`
pub(crate) fn attn_q_proj_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.self_attn.q_proj.weight")
}

/// `bert_layer.model.layers.{layer}.self_attn.k_proj.weight`
pub(crate) fn attn_k_proj_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.self_attn.k_proj.weight")
}

/// `bert_layer.model.layers.{layer}.self_attn.v_proj.weight`
pub(crate) fn attn_v_proj_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.self_attn.v_proj.weight")
}

/// `bert_layer.model.layers.{layer}.self_attn.out_proj.weight`
pub(crate) fn attn_out_proj_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.self_attn.out_proj.weight")
}

/// `bert_layer.model.layers.{layer}.self_attn.q_layernorm.weight`
pub(crate) fn attn_q_layernorm_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.self_attn.q_layernorm.weight")
}

/// `bert_layer.model.layers.{layer}.self_attn.k_layernorm.weight`
pub(crate) fn attn_k_layernorm_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.self_attn.k_layernorm.weight")
}

/// `bert_layer.model.layers.{layer}.conv.in_proj.weight`
pub(crate) fn conv_in_proj_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.conv.in_proj.weight")
}

/// `bert_layer.model.layers.{layer}.conv.out_proj.weight`
pub(crate) fn conv_out_proj_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.conv.out_proj.weight")
}

/// `bert_layer.model.layers.{layer}.conv.conv.weight` (shape `[hidden, 1, 3]`)
pub(crate) fn conv_conv_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.conv.conv.weight")
}

/// `bert_layer.model.layers.{layer}.feed_forward.w1.weight`
pub(crate) fn mlp_w1_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.feed_forward.w1.weight")
}

/// `bert_layer.model.layers.{layer}.feed_forward.w2.weight`
pub(crate) fn mlp_w2_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.feed_forward.w2.weight")
}

/// `bert_layer.model.layers.{layer}.feed_forward.w3.weight`
pub(crate) fn mlp_w3_weight(layer: usize) -> String {
    format!("{BACKBONE_PREFIX}layers.{layer}.feed_forward.w3.weight")
}

/// `bert_layer.layers_fuser.squeeze.weight`
pub(crate) fn fuser_squeeze_weight() -> String {
    format!("{ENCODER_PREFIX}layers_fuser.squeeze.weight")
}

/// `bert_layer.layers_fuser.squeeze.bias`
pub(crate) fn fuser_squeeze_bias() -> String {
    format!("{ENCODER_PREFIX}layers_fuser.squeeze.bias")
}

/// `bert_layer.layers_fuser.W1.weight`
pub(crate) fn fuser_w1_weight() -> String {
    format!("{ENCODER_PREFIX}layers_fuser.W1.weight")
}

/// `bert_layer.layers_fuser.W1.bias`
pub(crate) fn fuser_w1_bias() -> String {
    format!("{ENCODER_PREFIX}layers_fuser.W1.bias")
}

/// `bert_layer.layers_fuser.W2.weight`
pub(crate) fn fuser_w2_weight() -> String {
    format!("{ENCODER_PREFIX}layers_fuser.W2.weight")
}

/// `bert_layer.layers_fuser.W2.bias`
pub(crate) fn fuser_w2_bias() -> String {
    format!("{ENCODER_PREFIX}layers_fuser.W2.bias")
}

/// `bert_layer.layers_fuser.output_projection.weight`
pub(crate) fn fuser_output_projection_weight() -> String {
    format!("{ENCODER_PREFIX}layers_fuser.output_projection.weight")
}

/// `bert_layer.layers_fuser.output_projection.bias`
pub(crate) fn fuser_output_projection_bias() -> String {
    format!("{ENCODER_PREFIX}layers_fuser.output_projection.bias")
}

/// Strips the `bert_layer.model.` backbone prefix, if present.
pub(crate) fn strip_backbone_prefix(name: &str) -> Option<&str> {
    name.strip_prefix(BACKBONE_PREFIX)
}

/// Strips the `bert_layer.` encoder wrapper prefix, if present (covers both the
/// backbone and the `layers_fuser`).
pub(crate) fn strip_encoder_prefix(name: &str) -> Option<&str> {
    name.strip_prefix(ENCODER_PREFIX)
}

/// Validates that a checkpoint exposes every tensor the configuration implies,
/// using the full upstream names. Fails fast with the first missing name so a
/// mismatched `layer_types`/`fuse_layers` schema is caught before any weight is
/// read. The actual tensor reads use `VarBuilder::get` with the same names,
/// which additionally enforce shapes.
pub(crate) fn adapt_weights(vb: &VarBuilder, config: &Lfm2BiConfig) -> Result<()> {
    require(vb, &embed_tokens_weight())?;
    require(vb, &embedding_norm_weight())?;

    for (index, kind) in config.layer_types.iter().enumerate() {
        require(vb, &operator_norm_weight(index))?;
        require(vb, &ffn_norm_weight(index))?;
        require(vb, &mlp_w1_weight(index))?;
        require(vb, &mlp_w2_weight(index))?;
        require(vb, &mlp_w3_weight(index))?;
        match kind {
            LayerKind::Conv => {
                require(vb, &conv_in_proj_weight(index))?;
                require(vb, &conv_out_proj_weight(index))?;
                require(vb, &conv_conv_weight(index))?;
            }
            LayerKind::FullAttention => {
                require(vb, &attn_q_proj_weight(index))?;
                require(vb, &attn_k_proj_weight(index))?;
                require(vb, &attn_v_proj_weight(index))?;
                require(vb, &attn_out_proj_weight(index))?;
                require(vb, &attn_q_layernorm_weight(index))?;
                require(vb, &attn_k_layernorm_weight(index))?;
            }
        }
    }

    if config.fuse_layers {
        require(vb, &fuser_squeeze_weight())?;
        require(vb, &fuser_squeeze_bias())?;
        require(vb, &fuser_w1_weight())?;
        require(vb, &fuser_w1_bias())?;
        require(vb, &fuser_w2_weight())?;
        require(vb, &fuser_w2_bias())?;
        require(vb, &fuser_output_projection_weight())?;
        require(vb, &fuser_output_projection_bias())?;
    }

    Ok(())
}

fn require(vb: &VarBuilder, name: &str) -> Result<()> {
    if vb.contains_tensor(name) {
        Ok(())
    } else {
        candle_core::bail!(
            "LFM2 checkpoint is missing tensor `{name}`; \
             does not match the layer_types/fuse_layers configuration"
        )
    }
}

/// Builds a deterministic synthetic state dict matching `config` under the full
/// upstream names. Test-only; lets the model tests run without a checkpoint.
#[cfg(test)]
pub(crate) fn tiny_weights(config: &Lfm2BiConfig) -> HashMap<String, Tensor> {
    let device = &Device::Cpu;
    let hidden = config.hidden_size;
    let mut map = HashMap::new();

    map.insert(
        embed_tokens_weight(),
        rand_tensor(&[config.vocab_size, hidden], 1.0, device),
    );
    map.insert(
        embedding_norm_weight(),
        Tensor::ones((hidden,), DType::F32, device).unwrap(),
    );

    for (index, kind) in config.layer_types.iter().enumerate() {
        map.insert(
            operator_norm_weight(index),
            Tensor::ones((hidden,), DType::F32, device).unwrap(),
        );
        map.insert(
            ffn_norm_weight(index),
            Tensor::ones((hidden,), DType::F32, device).unwrap(),
        );
        map.insert(
            mlp_w1_weight(index),
            rand_tensor(&[config.intermediate_size, hidden], 0.5, device),
        );
        map.insert(
            mlp_w3_weight(index),
            rand_tensor(&[config.intermediate_size, hidden], 0.5, device),
        );
        map.insert(
            mlp_w2_weight(index),
            rand_tensor(&[hidden, config.intermediate_size], 0.5, device),
        );
        match kind {
            LayerKind::Conv => {
                map.insert(
                    conv_in_proj_weight(index),
                    rand_tensor(&[3 * hidden, hidden], 0.5, device),
                );
                map.insert(
                    conv_out_proj_weight(index),
                    rand_tensor(&[hidden, hidden], 0.5, device),
                );
                map.insert(
                    conv_conv_weight(index),
                    rand_tensor(&[hidden, 1, config.conv_l_cache], 0.5, device),
                );
            }
            LayerKind::FullAttention => {
                let q_out = config.num_attention_heads * config.head_dim;
                let kv_out = config.num_key_value_heads * config.head_dim;
                map.insert(
                    attn_q_proj_weight(index),
                    rand_tensor(&[q_out, hidden], 0.5, device),
                );
                map.insert(
                    attn_k_proj_weight(index),
                    rand_tensor(&[kv_out, hidden], 0.5, device),
                );
                map.insert(
                    attn_v_proj_weight(index),
                    rand_tensor(&[kv_out, hidden], 0.5, device),
                );
                map.insert(
                    attn_out_proj_weight(index),
                    rand_tensor(&[hidden, q_out], 0.5, device),
                );
                map.insert(
                    attn_q_layernorm_weight(index),
                    Tensor::ones((config.head_dim,), DType::F32, device).unwrap(),
                );
                map.insert(
                    attn_k_layernorm_weight(index),
                    Tensor::ones((config.head_dim,), DType::F32, device).unwrap(),
                );
            }
        }
    }

    if config.fuse_layers {
        let layers = config.num_hidden_layers;
        let half = layers / 2;
        map.insert(
            fuser_squeeze_weight(),
            rand_tensor(&[1, hidden], 0.5, device),
        );
        map.insert(fuser_squeeze_bias(), rand_tensor(&[1], 0.5, device));
        map.insert(fuser_w1_weight(), rand_tensor(&[half, layers], 0.5, device));
        map.insert(fuser_w1_bias(), rand_tensor(&[half], 0.5, device));
        map.insert(fuser_w2_weight(), rand_tensor(&[layers, half], 0.5, device));
        map.insert(fuser_w2_bias(), rand_tensor(&[layers], 0.5, device));
        map.insert(
            fuser_output_projection_weight(),
            rand_tensor(&[hidden, hidden], 0.5, device),
        );
        map.insert(
            fuser_output_projection_bias(),
            rand_tensor(&[hidden], 0.5, device),
        );
    }

    map
}

/// Deterministic pseudo-random fill (seeded LCG) so tests are reproducible.
#[cfg(test)]
fn rand_tensor(shape: &[usize], scale: f32, device: &Device) -> Tensor {
    let count: usize = shape.iter().product();
    let values: Vec<f32> = (0..count)
        .map(|index| {
            let state = (index as u64)
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let unit = ((state >> 33) as f32 / (1u64 << 31) as f32) - 0.5;
            unit * 2.0 * scale
        })
        .collect();
    Tensor::from_vec(values, shape, device).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_config() -> Lfm2BiConfig {
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
            fuse_layers: true,
            num_post_fusion_layers: 1,
            num_rnn_layers: 1,
        }
    }

    #[test]
    fn strip_backbone_prefix_removes_gliner_wrapper() {
        assert_eq!(
            strip_backbone_prefix("bert_layer.model.layers.0.operator_norm.weight"),
            Some("layers.0.operator_norm.weight")
        );
        assert_eq!(strip_backbone_prefix("model.embed_tokens.weight"), None);
        assert_eq!(
            strip_backbone_prefix("bert_layer.layers_fuser.0.weight"),
            None
        );
    }

    #[test]
    fn strip_encoder_prefix_handles_fuser_and_backbone() {
        assert_eq!(
            strip_encoder_prefix("bert_layer.layers_fuser.W1.weight"),
            Some("layers_fuser.W1.weight")
        );
        assert_eq!(
            strip_encoder_prefix("bert_layer.model.embed_tokens.weight"),
            Some("model.embed_tokens.weight")
        );
        assert_eq!(strip_encoder_prefix("embed_tokens.weight"), None);
    }

    #[test]
    fn name_builders_match_upstream_state_dict() {
        assert_eq!(
            embed_tokens_weight(),
            "bert_layer.model.embed_tokens.weight"
        );
        assert_eq!(
            embedding_norm_weight(),
            "bert_layer.model.embedding_norm.weight"
        );
        assert_eq!(
            operator_norm_weight(0),
            "bert_layer.model.layers.0.operator_norm.weight"
        );
        assert_eq!(
            ffn_norm_weight(15),
            "bert_layer.model.layers.15.ffn_norm.weight"
        );
        assert_eq!(
            attn_q_proj_weight(2),
            "bert_layer.model.layers.2.self_attn.q_proj.weight"
        );
        assert_eq!(
            attn_k_layernorm_weight(2),
            "bert_layer.model.layers.2.self_attn.k_layernorm.weight"
        );
        assert_eq!(
            conv_in_proj_weight(1),
            "bert_layer.model.layers.1.conv.in_proj.weight"
        );
        assert_eq!(
            conv_conv_weight(1),
            "bert_layer.model.layers.1.conv.conv.weight"
        );
        assert_eq!(
            mlp_w1_weight(0),
            "bert_layer.model.layers.0.feed_forward.w1.weight"
        );
        assert_eq!(
            mlp_w2_weight(0),
            "bert_layer.model.layers.0.feed_forward.w2.weight"
        );
        assert_eq!(
            fuser_squeeze_weight(),
            "bert_layer.layers_fuser.squeeze.weight"
        );
        assert_eq!(fuser_w2_bias(), "bert_layer.layers_fuser.W2.bias");
        assert_eq!(
            fuser_output_projection_weight(),
            "bert_layer.layers_fuser.output_projection.weight"
        );
    }

    #[test]
    fn adapt_weights_accepts_complete_tiny_checkpoint() {
        let config = tiny_config();
        let weights = tiny_weights(&config);
        let vb = VarBuilder::from_tensors(weights, DType::F32, &Device::Cpu);
        adapt_weights(&vb, &config).expect("complete tiny checkpoint passes adaptation");
    }

    #[test]
    fn adapt_weights_reports_first_missing_expected_tensor() {
        let config = tiny_config();
        let mut weights = tiny_weights(&config);
        weights.remove(&embed_tokens_weight());
        let vb = VarBuilder::from_tensors(weights, DType::F32, &Device::Cpu);
        let error = adapt_weights(&vb, &config).expect_err("missing embedding must fail");
        assert!(
            error.to_string().contains("embed_tokens.weight"),
            "unexpected error: {error}"
        );
    }
}
