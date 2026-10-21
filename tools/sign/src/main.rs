//! Служебная утилита релизов (не входит в установщик).
//!
//!   switchdesk-sign keygen
//!   switchdesk-sign sign ВЕРСИЯ ФАЙЛ      (секрет — в переменной UPDATE_SIGNING_KEY)
//!   switchdesk-sign verify ВЕРСИЯ ФАЙЛ    (проверка зашитым в программу ключом)
//!
//! `sign` пишет подпись рядом с файлом: `ФАЙЛ.sig` (64 байта).

use ed25519_dalek::{Signer, SigningKey};
use switchdesk_update::{decode_hex, encode_hex, signed_message, Version};

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["keygen"] => {
            let mut seed = [0u8; 32];
            getrandom::getrandom(&mut seed).unwrap_or_else(|e| die(&format!("нет случайности: {e}")));
            let sk = SigningKey::from_bytes(&seed);
            println!("UPDATE_SIGNING_KEY={}", encode_hex(&seed));
            println!("PUBLIC_KEY_HEX={}", encode_hex(sk.verifying_key().as_bytes()));
        }
        ["sign", version, file] => {
            let version = Version::parse(version).unwrap_or_else(|| die("версия должна быть вида X.Y.Z"));
            let seed: [u8; 32] = std::env::var("UPDATE_SIGNING_KEY")
                .ok()
                .and_then(|s| decode_hex(&s))
                .and_then(|b| b.try_into().ok())
                .unwrap_or_else(|| die("UPDATE_SIGNING_KEY не задан или не 32 байта в hex"));
            let data = std::fs::read(file).unwrap_or_else(|e| die(&format!("{file}: {e}")));
            let sig = SigningKey::from_bytes(&seed).sign(&signed_message(&version, &data));
            let out = format!("{file}.sig");
            std::fs::write(&out, sig.to_bytes()).unwrap_or_else(|e| die(&format!("{out}: {e}")));
            println!("подписано: {out}");
        }
        ["verify", version, file] => {
            // Проверка ключом, зашитым в программу: ловит несовпадение секрета и PUBLIC_KEY_HEX.
            let version = Version::parse(version).unwrap_or_else(|| die("версия должна быть вида X.Y.Z"));
            let data = std::fs::read(file).unwrap_or_else(|e| die(&format!("{file}: {e}")));
            let sig = std::fs::read(format!("{file}.sig")).unwrap_or_else(|e| die(&format!("{file}.sig: {e}")));
            switchdesk_update::verify(&version, &data, &sig).unwrap_or_else(|e| die(&e));
            println!("подпись верна");
        }
        _ => die("использование: switchdesk-sign keygen | sign ВЕРСИЯ ФАЙЛ | verify ВЕРСИЯ ФАЙЛ"),
    }
}
