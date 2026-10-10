//! Local XLM-R sequence-classification inference for multilingual retrieval.
//! Scores are the model's raw logits. No calibration on retrieval test outputs.

use crate::error::MemoryError;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::xlm_roberta::{Config, XLMRobertaForSequenceClassification};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};

mod qwen;

enum Backend {
    XlmRoberta(XLMRobertaForSequenceClassification),
    Qwen(qwen::QwenReranker),
}

pub(super) struct CandleReranker {
    model: Backend,
    tokenizer: tokenizers::Tokenizer,
    signature: String,
    admission_floor: f64,
}

fn inference_error(error: impl std::fmt::Display) -> MemoryError {
    MemoryError::Storage(format!("reranker inference failed: {error}"))
}

impl CandleReranker {
    pub(super) fn load(dir: &Path, admission_floor: f64) -> Result<Self, MemoryError> {
        if !admission_floor.is_finite() {
            return Err(MemoryError::ConfigInvalid(
                "RETRIEVAL_RERANKER_ADMISSION_FLOOR must be finite".into(),
            ));
        }
        let config_text =
            std::fs::read_to_string(dir.join("config.json")).map_err(inference_error)?;
        let metadata: serde_json::Value =
            serde_json::from_str(&config_text).map_err(inference_error)?;
        let is_qwen = metadata["model_type"] == "qwen3";
        if !is_qwen
            && (metadata["model_type"] != "xlm-roberta"
                || metadata["id2label"].as_object().map_or(0, |v| v.len()) != 1)
        {
            return Err(MemoryError::ConfigInvalid(
                "reranker must be a single-logit XLM-R classifier or Qwen3 causal reranker".into(),
            ));
        }
        let mut tokenizer = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(inference_error)?;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: 512,
                ..Default::default()
            }))
            .map_err(inference_error)?;
        tokenizer.with_padding(None);
        let is_gguf = dir
            .join("model.gguf")
            .try_exists()
            .map_err(inference_error)?;
        if is_gguf && !is_qwen {
            return Err(MemoryError::ConfigInvalid(
                "GGUF reranking is supported only for Qwen3".into(),
            ));
        }
        let weights_name = if is_gguf {
            "model.gguf"
        } else {
            "model.safetensors"
        };
        let weights_path = dir.join(weights_name);
        let model = if is_gguf {
            Backend::Qwen(qwen::QwenReranker::load_gguf(
                &config_text,
                &weights_path,
                &tokenizer,
            )?)
        } else {
            let device = Device::Cpu;
            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[&weights_path], DType::F32, &device)
            }
            .map_err(inference_error)?;
            if is_qwen {
                Backend::Qwen(qwen::QwenReranker::load(&config_text, vb, &tokenizer)?)
            } else {
                let config: Config = serde_json::from_str(&config_text).map_err(inference_error)?;
                Backend::XlmRoberta(
                    XLMRobertaForSequenceClassification::new(1, &config, vb)
                        .map_err(inference_error)?,
                )
            }
        };
        let mut hasher = Sha256::new();
        if is_qwen {
            qwen::update_input_signature(&mut hasher);
        } else {
            hasher.update(b"xlm-r-pair-512-source-reference-time-single-logit-floor-zero-v3");
        }
        if is_gguf {
            hasher.update(b"candle-qwen3-gguf-cpu-f32-v1");
        }
        for name in ["config.json", "tokenizer.json", weights_name] {
            hasher.update(name.as_bytes());
            let mut file = std::fs::File::open(dir.join(name)).map_err(inference_error)?;
            let mut buffer = [0_u8; 65536];
            loop {
                let count = file.read(&mut buffer).map_err(inference_error)?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
        }
        if admission_floor != 0.0 {
            hasher.update(b"independently-selected-admission-floor");
            hasher.update(admission_floor.to_le_bytes());
        }
        Ok(Self {
            model,
            tokenizer,
            signature: format!("reranksig:{}", hex::encode(hasher.finalize())),
            admission_floor,
        })
    }

    pub(super) fn signature(&self) -> &str {
        &self.signature
    }

    pub(super) fn admission_floor(&self) -> f64 {
        self.admission_floor
    }

    pub(super) fn score(&self, query: &str, documents: &[String]) -> Result<Vec<f64>, MemoryError> {
        let model = match &self.model {
            Backend::Qwen(model) => return model.score(&self.tokenizer, query, documents),
            Backend::XlmRoberta(model) => model,
        };
        let device = Device::Cpu;
        documents
            .iter()
            .map(|document| {
                let encoding = self
                    .tokenizer
                    .encode((query, document.as_str()), true)
                    .map_err(inference_error)?;
                let ids = Tensor::new(encoding.get_ids(), &device)
                    .map_err(inference_error)?
                    .unsqueeze(0)
                    .map_err(inference_error)?;
                let mask = Tensor::new(encoding.get_attention_mask(), &device)
                    .map_err(inference_error)?
                    .unsqueeze(0)
                    .map_err(inference_error)?;
                let types =
                    Tensor::zeros(ids.shape(), DType::U32, &device).map_err(inference_error)?;
                let output = model
                    .forward(&ids, &mask, &types)
                    .map_err(inference_error)?;
                let score = f64::from(
                    output
                        .flatten_all()
                        .map_err(inference_error)?
                        .to_vec1::<f32>()
                        .map_err(inference_error)?[0],
                );
                if !score.is_finite() {
                    return Err(MemoryError::Validation("non-finite reranker logit".into()));
                }
                Ok(score)
            })
            .collect()
    }
}
