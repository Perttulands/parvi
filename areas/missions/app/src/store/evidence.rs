//! Evidence paths and problems for a run, fetched independently of its live projection.
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub problems: Vec<Problem>,
    pub briefs: Vec<Brief>,
    pub files: Vec<File>,
    /// Files the run's agents wrote into its folder.
    #[serde(default)]
    pub written: Vec<Written>,
    #[serde(default)]
    pub truncated: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Written {
    pub path: String,
    pub size: u64,
}

impl Evidence {
    /// The files the run wrote that are not already among `outputs`, each
    /// as an output named by its path in the run's folder `cwd`.
    pub fn written_outputs(
        &self,
        cwd: &str,
        outputs: &[parvi_missions_contract::RunOutput],
    ) -> Vec<parvi_missions_contract::RunOutput> {
        let base = format!("{}/", cwd.trim_end_matches('/'));
        self.written
            .iter()
            .filter(|w| !outputs.iter().any(|o| o.value == w.path))
            .map(|w| parvi_missions_contract::RunOutput {
                name: w.path.strip_prefix(&base).unwrap_or(&w.path).to_owned(),
                kind: "file".into(),
                value: w.path.clone(),
                node_id: String::new(),
                port_id: String::new(),
                attempt: 0,
                picks: Vec::new(),
            })
            .collect()
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub seq: u64,
    pub node_id: String,
    pub reason: Text,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Text {
    pub text: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Brief {
    pub node_id: String,
    pub dispatch_seq: u64,
    pub path: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub node_id: String,
    pub path: String,
    pub size: u64,
}
#[derive(Deserialize)]
pub struct Response {
    pub evidence: Evidence,
}

#[derive(Default)]
pub struct Store {
    pub evidence: Option<Evidence>,
    pub read_at: Option<usize>,
    pub error: Option<String>,
    pub images: BTreeMap<String, Image>,
}

pub enum Image {
    Loading,
    Ready(Arc<gpui::RenderImage>),
    Failed(String),
}

pub fn image_format(path: &str) -> Option<gpui::ImageFormat> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => gpui::ImageFormat::Png,
        "jpg" | "jpeg" => gpui::ImageFormat::Jpeg,
        "webp" => gpui::ImageFormat::Webp,
        "gif" => gpui::ImageFormat::Gif,
        "svg" => gpui::ImageFormat::Svg,
        "bmp" => gpui::ImageFormat::Bmp,
        "tif" | "tiff" => gpui::ImageFormat::Tiff,
        "ico" => gpui::ImageFormat::Ico,
        _ => return None,
    })
}
