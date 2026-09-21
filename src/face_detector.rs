use std::path::{Path, PathBuf};

use anyhow::Result;
use image::imageops::FilterType;
use ort::session::Session;
use ort::value::Tensor;

const IMG_SIZE: u32 = 224;
const EMBED_DIM: usize = 512;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Match,
    Diffrent,
}

impl Verdict {
    pub fn as_status(self) -> &'static str {
        match self {
            Verdict::Match => "match",
            Verdict::Diffrent => "diffrent",
        }
    }
}

pub struct FaceDetector {
    session: Option<Session>,
    enabled: bool,
}

impl FaceDetector {
    pub fn new(model_dir: &str, enabled: bool) -> Result<Self> {
        if !enabled {
            return Ok(Self {
                session: None,
                enabled: false,
            });
        }

        let onnx_path = PathBuf::from(model_dir).join("model.onnx");
        if !onnx_path.exists() {
            anyhow::bail!("face ONNX model not found at {onnx_path:?}");
        }

        let session = Session::builder()?.commit_from_file(onnx_path)?;
        Ok(Self {
            session: Some(session),
            enabled: true,
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn embed_file(&mut self, path: &Path) -> Result<Vec<f32>> {
        let data = std::fs::read(path)?;
        let img = image::load_from_memory(&data)?.into_rgb8();
        self.embed_rgb(&img)
    }

    pub fn embed_rgb(&mut self, img: &image::RgbImage) -> Result<Vec<f32>> {
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("face detector is disabled"))?;

        let resized = image::imageops::resize(img, IMG_SIZE, IMG_SIZE, FilterType::Lanczos3);

        let plane = (IMG_SIZE * IMG_SIZE) as usize;
        let mut chw = vec![0f32; 3 * plane];
        for (x, y, px) in resized.enumerate_pixels() {
            let idx = (y * IMG_SIZE + x) as usize;
            let [r, g, b] = px.0;
            chw[idx] = (r as f32 / 255.0 - MEAN[0]) / STD[0];
            chw[plane + idx] = (g as f32 / 255.0 - MEAN[1]) / STD[1];
            chw[2 * plane + idx] = (b as f32 / 255.0 - MEAN[2]) / STD[2];
        }

        let shape = vec![1_i64, 3, IMG_SIZE as i64, IMG_SIZE as i64];
        let inputs = ort::inputs![
            "input" => Tensor::from_array((shape, chw))?
        ];
        let outputs = session.run(inputs)?;
        let (_, data) = outputs["embedding"].try_extract_tensor::<f32>()?;

        if data.len() != EMBED_DIM {
            anyhow::bail!(
                "unexpected embedding length: got {}, expected {EMBED_DIM}",
                data.len()
            );
        }

        let mut out = data.to_vec();
        l2_normalize(&mut out);
        Ok(out)
    }
}

pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    (1.0 - dot).clamp(0.0, 2.0)
}

pub fn distance_to_prototype(probe: &[f32], refs: &[Vec<f32>]) -> Option<f32> {
    if refs.is_empty() {
        return None;
    }
    let dim = probe.len();
    let mut proto = vec![0f32; dim];
    for r in refs {
        if r.len() != dim {
            return None;
        }
        for (p, x) in proto.iter_mut().zip(r) {
            *p += x;
        }
    }
    let n = refs.len() as f32;
    for p in proto.iter_mut() {
        *p /= n;
    }
    l2_normalize(&mut proto);
    Some(cosine_distance(probe, &proto))
}

pub fn classify(distance: f32, threshold: f32) -> Verdict {
    if distance < threshold {
        Verdict::Match
    } else {
        Verdict::Diffrent
    }
}

fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-12 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}
