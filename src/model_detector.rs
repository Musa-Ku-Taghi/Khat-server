use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;
use levenshtein::levenshtein;
use ort::session::Session;
use ort::value::Tensor;
use regex::Regex;
use serde::Deserialize;
use tokenizers::Tokenizer;
use tracing::debug;

const MAX_LEN: usize = 128;

pub struct ObfuscationProcessor {
    bad_words: HashSet<String>,
    patterns: Vec<(Regex, String)>,
    context_words: HashSet<String>,
    context_phrases: Vec<String>,
}

impl ObfuscationProcessor {
    pub fn new() -> Self {
        Self {
            bad_words: BAD_WORDS.iter().map(|s| s.to_string()).collect(),
            patterns: PATTERN_PAIRS
                .iter()
                .map(|(p, r)| {
                    (
                        Regex::new(p).expect("hard-coded regex must compile"),
                        r.to_string(),
                    )
                })
                .collect(),
            context_words: CONTEXT_WORDS.iter().map(|s| s.to_string()).collect(),
            context_phrases: CONTEXT_PHRASES.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn normalize(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (re, replacement) in &self.patterns {
            result = re.replace_all(&result, replacement.as_str()).into_owned();
        }
        result
    }

    pub fn has_obf(&self, text: &str) -> bool {
        let lower = text.to_lowercase();
        for (re, _) in &self.patterns {
            if re.is_match(&lower) && self.is_bad(&lower) {
                return true;
            }
        }
        for pat in GENERAL_PATTERNS {
            if Regex::new(pat)
                .expect("hard-coded regex must compile")
                .is_match(&lower)
                && self.is_bad(&lower)
            {
                return true;
            }
        }
        false
    }

    pub fn fuzzy(&self, text: &str, threshold: usize) -> bool {
        let norm_lower = self.normalize(text).to_lowercase();
        for w in text.split_whitespace() {
            if w.len() < 2 {
                continue;
            }
            for bad in &self.bad_words {
                if norm_lower.contains(bad.as_str()) && self.is_bad(&norm_lower) {
                    return true;
                }
                if bad.len() >= 2 && levenshtein(w, bad) <= threshold && self.is_bad(&norm_lower) {
                    return true;
                }
            }
        }
        false
    }

    pub fn process(&self, text: &str) -> String {
        let normalized = self.normalize(text);
        WHITESPACE_RE
            .replace_all(&normalized, " ")
            .trim()
            .to_string()
    }

    fn is_bad(&self, text: &str) -> bool {
        let t = text.to_lowercase();
        self.context_words.iter().any(|w| t.contains(w))
            || self.context_phrases.iter().any(|ph| t.contains(ph))
    }
}

impl Default for ObfuscationProcessor {
    fn default() -> Self {
        Self::new()
    }
}

static WHITESPACE_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"\s+").expect("hard-coded regex must compile"));

const BAD_WORDS: &[&str] = &[
    "کیر",
    "کص",
    "کصشر",
    "گایید",
    "گاییدم",
    "گاییده",
    "کونی",
    "جنده",
    "حرومزاده",
    "پدرسگ",
    "مادرجنده",
    "جاکش",
    "خارکصه",
    "کصخل",
    "کصکش",
    "کصمادر",
    "مادرکص",
    "پدرسگ",
    "مادر سگ",
    "مادرسگ",
    "کون",
    "کونده",
    "خواهرسگ",
    "پدرکصه",
    "خواهر سگ",
    "پدر سگ",
    "کونی",
];

const PATTERN_PAIRS: &[(&str, &str)] = &[
    (r"ک\*ر", "کیر"),
    (r"ک_یر", "کیر"),
    (r"ک-یر", "کیر"),
    (r"ک\.یر", "کیر"),
    (r"ک\s+یر", "کیر"),
    (r"ک\*یر", "کیر"),
    (r"ک\*ص", "کص"),
    (r"ک_ص", "کص"),
    (r"ک-ص", "کص"),
    (r"ک\.ص", "کص"),
    (r"کص\*شر", "کصشر"),
    (r"کص_شر", "کصشر"),
    (r"کص-شر", "کصشر"),
    (r"ک\*صشر", "کصشر"),
    (r"ک\*نی", "کونی"),
    (r"ک_ونی", "کونی"),
    (r"گ\*یید", "گایید"),
    (r"گ_ایید", "گایید"),
];

const CONTEXT_WORDS: &[&str] = &[
    "گایید",
    "کونی",
    "جنده",
    "کصه",
    "خواهرسگ",
    "کص",
    "کیر",
    "پدرسگ",
    "مادرجنده",
    "جاکش",
];

const CONTEXT_PHRASES: &[&str] = &[
    "کیرت",
    "کیرم",
    "کیرش",
    "کیرت تو",
    "کیر ک",
    "کص ننه",
    "کص مادر",
    "کصتو",
    "کصمادر",
    "گاییدم",
    "گاییده",
    "گاییدت",
];

const GENERAL_PATTERNS: &[&str] = &[
    r"ک\*[صیر]",
    r"ک_[صیر]",
    r"ک-[صیر]",
    r"ک\.[صیر]",
    r"کص\*",
    r"کص_",
    r"گ\*",
    r"گ_",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorKind {
    Model,
    ObfuscationDetector,
    Empty,
}

#[derive(Debug, Clone)]
pub struct Classification {
    pub label: String,
    pub detected_by: DetectorKind,
    pub confidence: f32,
}

impl Classification {
    pub fn should_block(&self) -> bool {
        match self.detected_by {
            DetectorKind::Model | DetectorKind::ObfuscationDetector => self.label != "clean",
            DetectorKind::Empty => false,
        }
    }
}

#[derive(Deserialize)]
struct HfConfig {
    #[serde(default)]
    id2label: HashMap<String, String>,
}

pub struct ModelDetector {
    session: Option<Session>,
    tokenizer: Option<Tokenizer>,
    processor: ObfuscationProcessor,
    id2label: HashMap<u32, String>,
    enabled: bool,
}

impl ModelDetector {
    pub fn new(model_dir: &str, enabled: bool) -> Result<Self> {
        if !enabled {
            return Ok(Self {
                session: None,
                tokenizer: None,
                processor: ObfuscationProcessor::new(),
                id2label: HashMap::new(),
                enabled: false,
            });
        }

        let onnx_path = PathBuf::from(model_dir).join("model.onnx");
        if !onnx_path.exists() {
            anyhow::bail!("ONNX model not found at {onnx_path:?}");
        }

        let config_path = PathBuf::from(model_dir).join("config.json");
        let config_raw = std::fs::read_to_string(&config_path)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {e}", config_path.display()))?;
        let hf_config: HfConfig = serde_json::from_str(&config_raw)
            .map_err(|e| anyhow::anyhow!("Failed to parse {}: {e}", config_path.display()))?;

        let mut id2label: HashMap<u32, String> = HashMap::new();
        for (k, v) in hf_config.id2label {
            let idx: u32 = k
                .parse()
                .map_err(|_| anyhow::anyhow!("config.json id2label key is not an integer: {k}"))?;
            id2label.insert(idx, v);
        }
        if id2label.is_empty() {
            anyhow::bail!(
                "config.json at {} has no id2label mapping; cannot determine class names",
                config_path.display()
            );
        }

        let tokenizer_path = PathBuf::from(model_dir).join("tokenizer.json");
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {e}"))?;

        let session = Session::builder()?.commit_from_file(onnx_path)?;

        Ok(Self {
            session: Some(session),
            tokenizer: Some(tokenizer),
            processor: ObfuscationProcessor::new(),
            id2label,
            enabled: true,
        })
    }

    pub fn check_text(&mut self, text: &str) -> Result<Classification> {
        if !self.enabled || text.is_empty() {
            return Ok(unknown());
        }

        if self.processor.has_obf(text) || self.processor.fuzzy(text, 2) {
            debug!("Obfuscation detected: {text}");
            return Ok(Classification {
                label: "obscene".to_string(),
                detected_by: DetectorKind::ObfuscationDetector,
                confidence: 1.0,
            });
        }

        let cleaned = self.processor.process(text);
        if cleaned.is_empty() {
            return Ok(unknown());
        }

        self.run_model(&cleaned)
    }

    fn run_model(&mut self, cleaned: &str) -> Result<Classification> {
        let tokenizer = self
            .tokenizer
            .as_ref()
            .expect("enabled detector must have a tokenizer");

        let encoding = tokenizer
            .encode(cleaned, true)
            .map_err(|e| anyhow::anyhow!("Tokenizer error: {e}"))?;

        let pad_token_id = tokenizer.token_to_id("<pad>").unwrap_or(0) as i64;

        let mut ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();
        if ids.len() > MAX_LEN {
            ids.truncate(MAX_LEN);
        } else {
            ids.resize(MAX_LEN, pad_token_id);
        }

        let attention_mask: Vec<i64> = ids
            .iter()
            .map(|&x| if x != pad_token_id { 1 } else { 0 })
            .collect();

        let shape = vec![1, MAX_LEN];
        let inputs = ort::inputs![
            "input_ids" => Tensor::from_array((shape.clone(), ids))?,
            "attention_mask" => Tensor::from_array((shape, attention_mask))?
        ];

        let session = self
            .session
            .as_mut()
            .expect("enabled detector must have a session");
        let outputs = session.run(inputs)?;
        let logits = outputs["logits"].try_extract_tensor::<f32>()?;
        let logits_slice = logits.1;

        let max = logits_slice
            .iter()
            .fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        let exp_sum: f32 = logits_slice.iter().map(|&x| (x - max).exp()).sum();
        let probs: Vec<f32> = logits_slice
            .iter()
            .map(|&x| (x - max).exp() / exp_sum)
            .collect();

        let (max_idx, max_prob) = probs
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap();

        let label = self
            .id2label
            .get(&(max_idx as u32))
            .cloned()
            .unwrap_or_else(|| format!("label_{max_idx}"));

        debug!("Model: label={label}, max_prob={max_prob:.3}");
        Ok(Classification {
            label,
            detected_by: DetectorKind::Model,
            confidence: *max_prob,
        })
    }
}

fn unknown() -> Classification {
    Classification {
        label: "UNKNOWN".to_string(),
        detected_by: DetectorKind::Empty,
        confidence: 0.0,
    }
}
