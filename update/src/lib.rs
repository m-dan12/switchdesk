//! Версия, проверка и установка обновлений switchdesk.
//!
//! Обновления берутся из GitHub Releases. Установщик (`switchdesk-setup.exe`) подписан
//! ed25519; публичный ключ зашит в программу. Без верной подписи ничего не запускается.

pub mod cli;
mod http;
mod install;

use ed25519_dalek::{Signature, VerifyingKey};

pub const REPO: &str = "m-dan12/switchdesk";
/// Версия этой сборки (общая для всего воркспейса).
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
pub const INSTALLER_ASSET: &str = "switchdesk-setup.exe";
pub const SIG_ASSET: &str = "switchdesk-setup.exe.sig";
/// Публичный ключ подписи релизов (hex, 32 байта). Пара к секрету `UPDATE_SIGNING_KEY`.
pub const PUBLIC_KEY_HEX: &str = "e63c9c43f511430ebb947aca1952e1dda5ac6833f2fc00e93e17a10c07271192";

const MAX_JSON: usize = 1024 * 1024;
const MAX_INSTALLER: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// `1.2.3` или `v1.2.3`. Любые суффиксы (`-rc1`) не принимаются.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.strip_prefix('v').unwrap_or(s);
        let mut it = s.split('.');
        let mut next = || it.next()?.parse::<u64>().ok();
        let v = Version(next()?, next()?, next()?);
        if it.next().is_some() {
            return None;
        }
        Some(v)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

pub fn current() -> Version {
    Version::parse(CURRENT).expect("версия пакета должна быть вида X.Y.Z")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub installer_url: String,
    pub sig_url: String,
}

fn release_url_ok(url: &str, tag: &str) -> bool {
    url.starts_with(&format!("https://github.com/{REPO}/releases/download/{tag}/"))
}

/// Разбирает ответ `GET /repos/{REPO}/releases/latest`.
pub fn parse_release(json: &[u8]) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("JSON: {e}"))?;
    if v["draft"].as_bool().unwrap_or(false) || v["prerelease"].as_bool().unwrap_or(false) {
        return Err("последний релиз — черновик или предварительный".into());
    }
    let tag = v["tag_name"].as_str().ok_or("нет tag_name")?;
    let version = Version::parse(tag).ok_or_else(|| format!("непонятная версия «{tag}»"))?;
    let asset = |name: &str| -> Result<String, String> {
        let url = v["assets"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["name"] == name))
            .and_then(|x| x["browser_download_url"].as_str())
            .ok_or_else(|| format!("в релизе {tag} нет файла {name}"))?;
        if !release_url_ok(url, tag) {
            return Err(format!("подозрительный адрес загрузки: {url}"));
        }
        Ok(url.to_owned())
    };
    Ok(Release { version, installer_url: asset(INSTALLER_ASSET)?, sig_url: asset(SIG_ASSET)? })
}

/// Что именно подписывается: версия привязана к файлу, чтобы старую сборку
/// нельзя было выдать за новую.
pub fn signed_message(version: &Version, file: &[u8]) -> Vec<u8> {
    let mut m = format!("switchdesk-release\n{version}\n").into_bytes();
    m.extend_from_slice(file);
    m
}

pub fn decode_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

pub fn encode_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn verify_with(key_hex: &str, version: &Version, file: &[u8], sig: &[u8]) -> Result<(), String> {
    let key: [u8; 32] = decode_hex(key_hex)
        .and_then(|k| k.try_into().ok())
        .ok_or("в программе не задан публичный ключ подписи")?;
    let key = VerifyingKey::from_bytes(&key).map_err(|e| format!("ключ: {e}"))?;
    let sig = Signature::from_slice(sig).map_err(|_| "подпись неверной длины")?;
    key.verify_strict(&signed_message(version, file), &sig)
        .map_err(|_| "подпись установщика не сходится".to_string())
}

pub fn verify(version: &Version, file: &[u8], sig: &[u8]) -> Result<(), String> {
    verify_with(PUBLIC_KEY_HEX, version, file, sig)
}

/// Спрашивает GitHub о последнем релизе. `Some`, только если он новее текущей версии.
pub fn check() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = http::get(&url, MAX_JSON).map_err(|e| format!("не удалось проверить обновления: {e}"))?;
    let rel = parse_release(&body)?;
    Ok((rel.version > current()).then_some(rel))
}

/// Скачивает установщик, проверяет подпись и возвращает путь к нему.
pub fn download(rel: &Release) -> Result<std::path::PathBuf, String> {
    let file = http::get(&rel.installer_url, MAX_INSTALLER).map_err(|e| format!("загрузка: {e}"))?;
    let sig = http::get(&rel.sig_url, 1024).map_err(|e| format!("загрузка подписи: {e}"))?;
    verify(&rel.version, &file, &sig)?;
    install::store(&file).map_err(|e| format!("не удалось сохранить установщик: {e}"))
}

pub use install::run_installer;

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn version_parse_and_order() {
        assert_eq!(Version::parse("v1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("0.10.0"), Some(Version(0, 10, 0)));
        assert!(Version::parse("1.2").is_none());
        assert!(Version::parse("1.2.3.4").is_none());
        assert!(Version::parse("1.2.3-rc1").is_none());
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
    }

    fn sample(tag: &str, url_tag: &str) -> Vec<u8> {
        format!(
            r#"{{"tag_name":"{tag}","draft":false,"prerelease":false,"assets":[
              {{"name":"switchdesk-setup.exe","browser_download_url":"https://github.com/{REPO}/releases/download/{url_tag}/switchdesk-setup.exe"}},
              {{"name":"switchdesk-setup.exe.sig","browser_download_url":"https://github.com/{REPO}/releases/download/{url_tag}/switchdesk-setup.exe.sig"}}]}}"#
        )
        .into_bytes()
    }

    #[test]
    fn parses_release() {
        let r = parse_release(&sample("v0.2.0", "v0.2.0")).unwrap();
        assert_eq!(r.version, Version(0, 2, 0));
        assert!(r.installer_url.ends_with("/v0.2.0/switchdesk-setup.exe"));
    }

    #[test]
    fn rejects_foreign_or_mismatched_urls() {
        assert!(parse_release(&sample("v0.2.0", "v0.1.0")).is_err());
        let evil = String::from_utf8(sample("v0.2.0", "v0.2.0"))
            .unwrap()
            .replace("github.com/m-dan12", "evil.example/x");
        assert!(parse_release(evil.as_bytes()).is_err());
    }

    #[test]
    fn rejects_prerelease_and_missing_assets() {
        let pre = String::from_utf8(sample("v0.2.0", "v0.2.0")).unwrap().replace("\"prerelease\":false", "\"prerelease\":true");
        assert!(parse_release(pre.as_bytes()).is_err());
        assert!(parse_release(br#"{"tag_name":"v1.0.0","assets":[]}"#).is_err());
    }

    #[test]
    fn signature_binds_version_and_file() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let key_hex = encode_hex(sk.verifying_key().as_bytes());
        let v = Version(0, 2, 0);
        let file = b"installer bytes";
        let sig = sk.sign(&signed_message(&v, file)).to_bytes();

        assert!(verify_with(&key_hex, &v, file, &sig).is_ok());
        assert!(verify_with(&key_hex, &Version(0, 3, 0), file, &sig).is_err(), "другая версия");
        assert!(verify_with(&key_hex, &v, b"tampered", &sig).is_err(), "другой файл");
        assert!(verify_with(&key_hex, &v, file, &sig[..63]).is_err(), "короткая подпись");
        let other = SigningKey::from_bytes(&[8u8; 32]);
        assert!(verify_with(&encode_hex(other.verifying_key().as_bytes()), &v, file, &sig).is_err());
        assert!(verify_with("ff14f05700079f9d35841e248e90cb60d4920d6b067999de78e970fec5f39892", &v, file, &sig).is_err());
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(decode_hex(&encode_hex(&[0, 1, 254, 255])), Some(vec![0, 1, 254, 255]));
        assert!(decode_hex("abc").is_none());
        assert!(decode_hex("zz").is_none());
    }
}
