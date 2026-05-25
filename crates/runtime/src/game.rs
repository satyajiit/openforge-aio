//! The `Game` trait + `GameMeta` DTO used by IPC.

use serde::Serialize;

use crate::feature::{DeclFeatureSrc, Feature};

pub trait Game: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn tagline(&self) -> &'static str {
        ""
    }
    fn process_names(&self) -> &'static [&'static str];
    fn primary_module(&self) -> Option<&'static str> {
        None
    }
    fn supported_versions(&self) -> &'static [&'static str] {
        &[]
    }
    fn forbidden_services(&self) -> &'static [&'static str] {
        &[]
    }
    fn icon_png(&self) -> &'static [u8] {
        &[]
    }
    fn requires_admin(&self) -> bool {
        true
    }
    fn sort_order(&self) -> i32 {
        1000
    }
    fn declarative_features(&self) -> &'static [DeclFeatureSrc] {
        &[]
    }
    fn custom_features(&self) -> &'static [&'static dyn Feature] {
        &[]
    }
    /// File name of the per-game DLL that the trainer injects into the
    /// running game on Activate. Sourced from `manifest.toml`'s
    /// `dll_file_name` field via the game crate's `build.rs`. Empty means
    /// the game ships no DLL (rare; reserved for future engines that don't
    /// need in-process reflection).
    fn dll_file_name(&self) -> &'static str {
        ""
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GameMeta {
    pub id: String,
    pub display_name: String,
    pub tagline: String,
    pub process_names: Vec<String>,
    pub primary_module: Option<String>,
    pub supported_versions: Vec<String>,
    pub forbidden_services: Vec<String>,
    pub requires_admin: bool,
    pub sort_order: i32,
    /// PNG bytes encoded as a `data:image/png;base64,...` URL for direct use in
    /// `<img src=...>`. Empty string if no icon.
    pub icon_data_url: String,
}

impl GameMeta {
    pub fn from_game(g: &dyn Game) -> Self {
        Self {
            id: g.id().to_string(),
            display_name: g.display_name().to_string(),
            tagline: g.tagline().to_string(),
            process_names: g.process_names().iter().map(|s| s.to_string()).collect(),
            primary_module: g.primary_module().map(str::to_string),
            supported_versions: g
                .supported_versions()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            forbidden_services: g
                .forbidden_services()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            requires_admin: g.requires_admin(),
            sort_order: g.sort_order(),
            icon_data_url: encode_icon(g.icon_png()),
        }
    }
}

fn encode_icon(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let mut url = String::from("data:image/png;base64,");
    base64_encode_into(bytes, &mut url);
    url
}

fn base64_encode_into(bytes: &[u8], out: &mut String) {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let a = bytes[i];
        let b = bytes[i + 1];
        let c = bytes[i + 2];
        out.push(TABLE[(a >> 2) as usize] as char);
        out.push(TABLE[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        out.push(TABLE[(((b & 0x0F) << 2) | (c >> 6)) as usize] as char);
        out.push(TABLE[(c & 0x3F) as usize] as char);
        i += 3;
    }
    match bytes.len() - i {
        1 => {
            let a = bytes[i];
            out.push(TABLE[(a >> 2) as usize] as char);
            out.push(TABLE[((a & 0x03) << 4) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let a = bytes[i];
            let b = bytes[i + 1];
            out.push(TABLE[(a >> 2) as usize] as char);
            out.push(TABLE[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
            out.push(TABLE[((b & 0x0F) << 2) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
}
