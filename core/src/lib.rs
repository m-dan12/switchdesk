//! Ядро switchdesk: настройки, состояние и движок, который одновременно принимает ввод
//! от других компьютеров (хост) и отправляет ваш ввод на выбранный компьютер (клиент).

pub mod host;
pub mod settings;
pub mod state;

#[cfg(any(windows, switchdesk_check))]
mod client_net;
#[cfg(any(windows, switchdesk_check))]
mod engine;
#[cfg(any(windows, switchdesk_check))]
mod platform;

#[cfg(any(windows, switchdesk_check))]
pub use engine::Engine;
