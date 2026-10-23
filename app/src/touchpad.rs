//! Тачпад Precision Touchpad: прокрутка двумя пальцами.
//!
//! Windows превращает движение двух пальцев в прокрутку внутри самой системы, мимо хуков мыши,
//! поэтому на хост она не попадает. Читаем сырые отчёты тачпада (HID, страница 0x0D) и сами
//! переводим смещение центра двух пальцев в колесо мыши.

use crate::proto::Ev;
use std::collections::HashMap;
use windows::Win32::Devices::HumanInterfaceDevice::*;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::UI::Input::{GetRawInputDeviceInfoW, RIDI_PREPARSEDDATA};

const PAGE_GENERIC: u16 = 0x01;
const PAGE_DIGITIZER: u16 = 0x0D;
const USAGE_X: u16 = 0x30;
const USAGE_Y: u16 = 0x31;
const USAGE_TIP: u16 = 0x42;
const USAGE_CONTACT_ID: u16 = 0x51;
const USAGE_CONTACT_COUNT: u16 = 0x54;
/// Полная высота тачпада = столько делений колеса (по 120).
const NOTCHES_PER_PAD: f64 = 6.0;

struct Pad {
    pre: Vec<u8>,
    /// Коллекции пальцев (слоты) в отчёте.
    slots: Vec<u16>,
    x_max: f64,
    y_max: f64,
}

#[derive(Default)]
pub struct Touchpads {
    pads: HashMap<isize, Option<Pad>>,
    fingers: HashMap<u32, (f64, f64)>,
    /// Кадр, который собирается из нескольких отчётов, и сколько контактов в нём ещё не пришло.
    frame: HashMap<u32, (f64, f64)>,
    remaining: usize,
    logged: usize,
    prev: Option<(f64, f64)>,
    rest: (f64, f64),
    natural: Option<bool>,
}

fn load(device: HANDLE) -> Option<Pad> {
    unsafe {
        let mut size = 0u32;
        GetRawInputDeviceInfoW(Some(device), RIDI_PREPARSEDDATA, None, &mut size);
        if size == 0 {
            return None;
        }
        let mut pre = vec![0u8; size as usize];
        let got = GetRawInputDeviceInfoW(Some(device), RIDI_PREPARSEDDATA, Some(pre.as_mut_ptr() as *mut _), &mut size);
        if got == u32::MAX {
            return None;
        }
        let ppd = PHIDP_PREPARSED_DATA(pre.as_ptr() as isize);
        let mut caps = HIDP_CAPS::default();
        if HidP_GetCaps(ppd, &mut caps).0 < 0 {
            return None;
        }
        let mut n = caps.NumberInputValueCaps;
        let mut vc = vec![HIDP_VALUE_CAPS::default(); n as usize];
        if HidP_GetValueCaps(HidP_Input, vc.as_mut_ptr(), &mut n, ppd).0 < 0 {
            return None;
        }
        vc.truncate(n as usize);
        let mut slots = Vec::new();
        let (mut x_max, mut y_max) = (0.0, 0.0);
        for c in &vc {
            if c.IsRange || c.UsagePage != PAGE_GENERIC {
                continue;
            }
            let usage = c.Anonymous.NotRange.Usage;
            if usage == USAGE_X {
                slots.push(c.LinkCollection);
                x_max = c.LogicalMax as f64;
            } else if usage == USAGE_Y {
                y_max = c.LogicalMax as f64;
            }
        }
        // Это тачпад, если есть пальцы с координатами и идентификатором контакта.
        let has_id = vc
            .iter()
            .any(|c| c.UsagePage == PAGE_DIGITIZER && !c.IsRange && c.Anonymous.NotRange.Usage == USAGE_CONTACT_ID);
        if slots.is_empty() || !has_id || x_max <= 0.0 || y_max <= 0.0 {
            return None;
        }
        Some(Pad { pre, slots, x_max, y_max })
    }
}

/// Направление прокрутки берётся из настроек Windows («Вниз — прокрутка вниз» = естественная).
fn natural_scroll() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut v = 0u32;
    let mut len = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\PrecisionTouchPad"),
            w!("ScrollDirection"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut len),
        )
    };
    r.is_ok() && v != 0
}

impl Touchpads {
    /// Сбросить состояние пальцев (передача выключена или включена заново).
    pub fn reset(&mut self) {
        self.fingers.clear();
        self.frame.clear();
        self.remaining = 0;
        self.prev = None;
        self.rest = (0.0, 0.0);
        self.natural = None;
    }

    /// Разбирает WM_INPUT от HID-устройства; возвращает события колеса.
    pub fn feed(&mut self, device: HANDLE, size_hid: usize, reports: &[u8]) -> Vec<Ev> {
        let pad = self.pads.entry(device.0 as isize).or_insert_with(|| {
            let p = load(device);
            match &p {
                Some(p) => crate::capture::log(&format!("touchpad: слотов {}, X до {}, Y до {}", p.slots.len(), p.x_max, p.y_max)),
                None => crate::capture::log("touchpad: устройство не распознано как Precision Touchpad"),
            }
            p
        });
        let Some(pad) = pad.as_ref() else { return Vec::new() };
        let natural = *self.natural.get_or_insert_with(natural_scroll);
        let mut out = Vec::new();
        if size_hid == 0 {
            return out;
        }
        let ppd = PHIDP_PREPARSED_DATA(pad.pre.as_ptr() as isize);
        for rep in reports.chunks_exact(size_hid) {
            let report = rep.to_vec();
            // Счётчик контактов есть только в первом отчёте кадра; остальные пальцы идут следом.
            let mut count = 0u32;
            let ok = |lc: Option<u16>, count: &mut u32| unsafe {
                HidP_GetUsageValue(HidP_Input, PAGE_DIGITIZER, lc, USAGE_CONTACT_COUNT, count, ppd, &report).0 >= 0
            };
            if !ok(Some(0), &mut count) {
                ok(None, &mut count);
            }
            if count > 0 {
                self.frame.clear();
                self.remaining = count as usize;
            }
            if count == 0 && self.remaining == 0 {
                // Все пальцы подняты.
                self.fingers.clear();
            } else {
                let take = self.remaining.min(pad.slots.len());
                for &lc in &pad.slots[..take] {
                    let get = |page: u16, usage: u16| -> Option<u32> {
                        let mut v = 0u32;
                        let st = unsafe { HidP_GetUsageValue(HidP_Input, page, Some(lc), usage, &mut v, ppd, &report) };
                        (st.0 >= 0).then_some(v)
                    };
                    let (Some(id), Some(x), Some(y)) =
                        (get(PAGE_DIGITIZER, USAGE_CONTACT_ID), get(PAGE_GENERIC, USAGE_X), get(PAGE_GENERIC, USAGE_Y))
                    else {
                        continue;
                    };
                    let mut list = [0u16; 8];
                    let mut n = list.len() as u32;
                    let mut rb = report.clone();
                    let st = unsafe { HidP_GetUsages(HidP_Input, PAGE_DIGITIZER, Some(lc), list.as_mut_ptr(), &mut n, ppd, &mut rb) };
                    if st.0 >= 0 && list[..n as usize].contains(&USAGE_TIP) {
                        self.frame.insert(id, (x as f64, y as f64));
                    }
                }
                self.remaining -= take;
                if self.remaining > 0 {
                    continue; // кадр ещё не весь
                }
                self.fingers = std::mem::take(&mut self.frame);
            }

            if self.fingers.len() != self.logged {
                self.logged = self.fingers.len();
                crate::capture::log(&format!("touchpad: пальцев {}", self.logged));
            }
            if self.fingers.len() != 2 {
                self.prev = None;
                self.rest = (0.0, 0.0);
                continue;
            }
            let (mut cx, mut cy) = (0.0, 0.0);
            for (x, y) in self.fingers.values() {
                cx += x / 2.0;
                cy += y / 2.0;
            }
            if let Some((px, py)) = self.prev {
                // Сначала пальцы вниз = колесо вверх (классическое направление).
                let sign = if natural { -1.0 } else { 1.0 };
                self.rest.0 += (cx - px) / pad.x_max * 120.0 * NOTCHES_PER_PAD * sign;
                self.rest.1 += (cy - py) / pad.y_max * 120.0 * NOTCHES_PER_PAD * sign;
                let (h, v) = (self.rest.0.trunc(), self.rest.1.trunc());
                self.rest.0 -= h;
                self.rest.1 -= v;
                if v != 0.0 {
                    out.push(Ev::Wheel { v: v as i32, h: false });
                }
                if h != 0.0 {
                    out.push(Ev::Wheel { v: h as i32, h: true });
                }
            }
            self.prev = Some((cx, cy));
        }
        out
    }
}
