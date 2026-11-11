//! Static graphics-card identity shown by the read-only Graphics window.

use super::gpu::GpuDevice;

/// One read-only row in the Graphics window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuGraphicsRow {
    pub name: String,
    pub value: String,
    pub is_banded: bool,
}

/// Values describe the installed card and driver; unlike telemetry they do not change while the
/// application is running.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GpuGraphicsInfo {
    pub gpu: String,
    pub board_manufacturer: String,
    pub code_name: String,
    pub revision: String,
    /// `VVVV DDDD - SSSS ssss`: vendor and device, then board vendor and board id (GPU-Z's order).
    pub device_id: String,
    pub cores: String,
    pub rops_tmus: String,
    /// The driver's base and boost clocks, as read when the window opened.
    pub clocks: String,
    /// ROPs and TMUs at the boost clock.
    pub fill_rate: String,
    /// The default board power limit.
    pub tdp: String,
    pub technology: String,
    pub memory_size: String,
    pub memory_type: String,
    pub memory_vendor: String,
    pub bus_width: String,
    pub bandwidth: String,
    pub bus_interface: String,
    pub resizable_bar: String,
    pub bios_version: String,
    pub bus_address: String,
    pub driver_version: String,
    pub driver_date: String,
}

impl GpuGraphicsInfo {
    pub fn from_device(device: &GpuDevice) -> Self {
        Self {
            gpu: device.name.clone(),
            bios_version: device.bios_version.clone(),
            bus_address: device.bus_id.clone(),
            memory_size: if device.vram_megabytes > 0 {
                format_gb(device.vram_megabytes as f64 / 1024.0)
            } else {
                String::new()
            },
            driver_version: device.driver_version.clone(),
            driver_date: windows_display_identity::driver_date(&device.vendor, &device.driver_version),
            ..Default::default()
        }
    }

    /// The same field order as Roch Viewer's System Info / Graphics section.
    pub fn rows(&self) -> Vec<GpuGraphicsRow> {
        let values: [(&str, &str); 22] = [
            ("GPU", &self.gpu),
            ("Board Manufacturer", &self.board_manufacturer),
            ("GPU Code Name", &self.code_name),
            ("GPU Revision", &self.revision),
            ("Device ID", &self.device_id),
            ("Cores", &self.cores),
            ("ROPs / TMUs", &self.rops_tmus),
            ("Base / Boost Clock", &self.clocks),
            ("Pixel / Texture Fill", &self.fill_rate),
            ("TDP", &self.tdp),
            ("GPU Technology", &self.technology),
            ("Memory Size", &self.memory_size),
            ("Memory Type", &self.memory_type),
            ("Memory Vendor", &self.memory_vendor),
            ("Bus Width", &self.bus_width),
            ("Bandwidth", &self.bandwidth),
            ("Bus Interface", &self.bus_interface),
            ("Resizable BAR", &self.resizable_bar),
            ("VBIOS", &self.bios_version),
            ("PCI Bus Address", &self.bus_address),
            ("Driver Version", &self.driver_version),
            ("Driver Date", &self.driver_date),
        ];
        values
            .iter()
            .enumerate()
            .map(|(i, (name, value))| GpuGraphicsRow {
                name: (*name).to_string(),
                value: Self::display(value),
                is_banded: i % 2 == 1,
            })
            .collect()
    }

    /// Blank values show as an em dash.
    pub fn display(value: &str) -> String {
        let t = value.trim();
        if t.is_empty() { "—".to_string() } else { t.to_string() }
    }
}

/// `{0:0.##}` formatting: up to two decimals, trailing zeros dropped.
pub fn format_gb(gb: f64) -> String {
    format!("{} GB", trim_decimals(gb, 2))
}

/// Format with at most `digits` decimals and no trailing zeros (.NET "0.##"), rounding the way
/// .NET's custom formats do (see [`crate::util::dotnet_custom`]).
pub fn trim_decimals(value: f64, digits: usize) -> String {
    crate::util::dotnet_custom(value, 0, digits)
}

/// Small shared decoders for identifiers read by the vendor backends.
pub struct GpuIdentity;

impl GpuIdentity {
    pub fn board_vendor(id: i32) -> String {
        let name = match id {
            0x1043 => "ASUSTeK Computer",
            0x1458 => "GIGABYTE Technology",
            0x1462 => "MSI",
            0x148C => "PowerColor",
            0x1682 => "XFX",
            0x1849 => "ASRock",
            0x8086 => "Intel",
            0x196E => "PNY",
            0x19DA => "Zotac",
            0x1B4C => "KFA2",
            0x1DA2 => "Sapphire Technology",
            0x3842 => "EVGA",
            0x7377 => "Colorful",
            0x10DE => "NVIDIA",
            0x1002 => "AMD",
            _ => return if id > 0 { format!("0x{id:04X}") } else { String::new() },
        };
        name.to_string()
    }

    /// "20260115" or a parseable date → "2026-01-15"; anything else → "".
    pub fn driver_date_text(value: Option<&str>) -> String {
        let raw = value.unwrap_or("").trim();
        let bytes = raw.as_bytes();
        if bytes.len() >= 8 && bytes[..8].iter().all(|b| b.is_ascii_digit()) {
            return format!("{}-{}-{}", &raw[..4], &raw[4..6], &raw[6..8]);
        }
        parse_loose_date(raw).unwrap_or_default()
    }

    /// GPU-Z's device id: `10DE 2D04 - 1462 5351`. Each word packs its device over its vendor, as
    /// PCI configuration space does. Empty when the driver gave no ids.
    pub fn device_id_text(vendor_device: u32, subsystem: u32) -> String {
        if vendor_device == 0 {
            return String::new();
        }
        let word = |w: u32| format!("{:04X} {:04X}", w & 0xFFFF, w >> 16);
        if subsystem == 0 { word(vendor_device) } else { format!("{} - {}", word(vendor_device), word(subsystem)) }
    }

    /// `2407 / 2572 MHz`; one side alone when the other is unknown.
    pub fn clocks_text(base_mhz: f64, boost_mhz: f64) -> String {
        match (base_mhz > 0.0, boost_mhz > 0.0) {
            (true, true) => format!("{} / {} MHz", base_mhz.round(), boost_mhz.round()),
            (true, false) => format!("{} MHz", base_mhz.round()),
            (false, true) => format!("{} MHz boost", boost_mhz.round()),
            (false, false) => String::new(),
        }
    }

    /// `123.5 GPixel/s / 370.4 GTexel/s`: every ROP and TMU once per clock at the boost clock.
    pub fn fill_rate_text(rops: i32, tmus: i32, boost_mhz: f64) -> String {
        if rops <= 0 || tmus <= 0 || boost_mhz <= 0.0 {
            return String::new();
        }
        let giga = |units: i32| format!("{:.1}", units as f64 * boost_mhz / 1000.0);
        format!("{} GPixel/s / {} GTexel/s", giga(rops), giga(tmus))
    }

    /// Peak memory bandwidth from the transfer rate per pin and the bus width: `448.0 GB/s`.
    pub fn bandwidth_text(transfers_mt_s: f64, bus_bits: u32) -> String {
        if transfers_mt_s <= 0.0 || bus_bits == 0 {
            return String::new();
        }
        format!("{:.1} GB/s", transfers_mt_s * bus_bits as f64 / 8.0 / 1000.0)
    }

    /// `180 W`.
    pub fn watts_text(watts: f64) -> String {
        if watts.is_finite() && watts > 0.0 { format!("{} W", watts.round()) } else { String::new() }
    }

    /// Maximum PCIe link capability. The currently power-saving link is deliberately omitted so
    /// the header stays stable while the GPU idles; it is only the fallback.
    pub fn pcie_bus_interface(
        maximum_generation: i32,
        maximum_width: i32,
        current_generation: i32,
        current_width: i32,
    ) -> String {
        fn half(generation: i32, width: i32) -> String {
            if generation <= 0 && width <= 0 {
                return String::new();
            }
            if generation <= 0 {
                return format!("x{width}");
            }
            if width <= 0 {
                return format!("{generation}.0");
            }
            format!("{generation}.0 x{width}")
        }
        let maximum = half(maximum_generation, maximum_width);
        if !maximum.is_empty() { maximum } else { half(current_generation, current_width) }
    }
}

/// Best-effort date parsing for the formats Windows' registry and vendor APIs hand back:
/// "M-D-YYYY", "M/D/YYYY", "YYYY-MM-DD", optionally followed by a time.
fn parse_loose_date(raw: &str) -> Option<String> {
    let date_part = raw.split_whitespace().next()?;
    let parts: Vec<&str> = date_part.split(['-', '/', '.']).collect();
    if parts.len() != 3 {
        return None;
    }
    let nums: Vec<u32> = parts.iter().map(|p| p.parse::<u32>()).collect::<std::result::Result<_, _>>().ok()?;
    let (y, m, d) = if parts[0].len() == 4 {
        (nums[0], nums[1], nums[2])
    } else if parts[2].len() == 4 {
        (nums[2], nums[0], nums[1])
    } else {
        return None;
    };
    chrono::NaiveDate::from_ymd_opt(y as i32, m, d).map(|dt| dt.format("%Y-%m-%d").to_string())
}

/// Best-effort installed-driver metadata from Windows' display-class registry.
pub mod windows_display_identity {
    #[cfg(windows)]
    pub fn driver_date(vendor: &str, version: &str) -> String {
        use winreg::RegKey;
        use winreg::enums::HKEY_LOCAL_MACHINE;

        const DISPLAY_CLASS: &str = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

        let contains = |hay: &str, needle: &str| hay.to_lowercase().contains(&needle.to_lowercase());
        let Ok(root) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(DISPLAY_CLASS) else {
            return String::new();
        };
        let mut fallback_date = String::new();
        for child_name in root.enum_keys().flatten() {
            let Ok(child) = root.open_subkey(&child_name) else { continue };
            let provider: String = child.get_value("ProviderName").unwrap_or_default();
            let installed_version: String = child.get_value("DriverVersion").unwrap_or_default();
            let vendor_match = if vendor.eq_ignore_ascii_case("NVIDIA") {
                contains(&provider, "NVIDIA")
            } else if vendor.eq_ignore_ascii_case("Intel") {
                contains(&provider, "Intel")
            } else {
                vendor.eq_ignore_ascii_case("AMD")
                    && (contains(&provider, "AMD") || contains(&provider, "Advanced Micro Devices"))
            };
            let version_match = !version.is_empty() && contains(&installed_version, version);
            if !vendor_match {
                continue;
            }
            let raw: Option<String> = child.get_value("DriverDate").ok();
            let date = super::GpuIdentity::driver_date_text(raw.as_deref());
            if date.is_empty() {
                continue;
            }
            if version_match {
                return date;
            }
            if fallback_date.is_empty() {
                fallback_date = date;
            }
        }
        // Intel may have an integrated GPU with a different driver installed. Never use its date
        // for an Arc card when the requested version was not found.
        if vendor.eq_ignore_ascii_case("Intel") && !version.is_empty() { String::new() } else { fallback_date }
    }

    #[cfg(not(windows))]
    pub fn driver_date(_vendor: &str, _version: &str) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_decoders() {
        assert_eq!(GpuIdentity::board_vendor(0x1849), "ASRock");
        assert_eq!(GpuIdentity::board_vendor(0x1234), "0x1234");
        assert_eq!(GpuIdentity::board_vendor(0), "");
        assert_eq!(GpuIdentity::driver_date_text(Some("20260115")), "2026-01-15");
        assert_eq!(GpuIdentity::driver_date_text(Some("8-5-2025")), "2025-08-05");
        assert_eq!(GpuIdentity::driver_date_text(Some("nonsense")), "");
        assert_eq!(GpuIdentity::pcie_bus_interface(5, 16, 1, 16), "5.0 x16");
        assert_eq!(GpuIdentity::pcie_bus_interface(0, 0, 4, 8), "4.0 x8");
        assert_eq!(GpuIdentity::pcie_bus_interface(0, 0, 0, 0), "");
        assert_eq!(format_gb(12282.0 / 1024.0), "11.99 GB");
        assert_eq!(format_gb(16.0), "16 GB");
        // What GPU-Z and CPU-Z show for an MSI RTX 5060 Ti (NVAPI packs device over vendor).
        assert_eq!(GpuIdentity::device_id_text(0x2D04_10DE, 0x5351_1462), "10DE 2D04 - 1462 5351");
        assert_eq!(GpuIdentity::device_id_text(0x2D04_10DE, 0), "10DE 2D04");
        assert_eq!(GpuIdentity::device_id_text(0, 0), "");
        assert_eq!(GpuIdentity::clocks_text(2407.0, 2572.0), "2407 / 2572 MHz");
        assert_eq!(GpuIdentity::clocks_text(0.0, 0.0), "");
        assert_eq!(GpuIdentity::fill_rate_text(48, 144, 2572.0), "123.5 GPixel/s / 370.4 GTexel/s");
        assert_eq!(GpuIdentity::fill_rate_text(0, 144, 2572.0), "");
        assert_eq!(GpuIdentity::bandwidth_text(28_002.0, 128), "448.0 GB/s");
        assert_eq!(GpuIdentity::bandwidth_text(28_002.0, 0), "");
        assert_eq!(GpuIdentity::watts_text(180.0), "180 W");
        assert_eq!(GpuIdentity::watts_text(f64::NAN), "");
        // An exact tie rounds away from zero, as .NET's "0.##" does (12416 MB).
        assert_eq!(format_gb(12.125), "12.13 GB");
    }

    #[test]
    fn rows_band_and_dash() {
        let rows = GpuGraphicsInfo { gpu: "X".into(), ..Default::default() }.rows();
        assert_eq!(rows.len(), 22);
        assert_eq!(rows[0].value, "X");
        assert!(!rows[0].is_banded && rows[1].is_banded);
        assert_eq!(rows[1].value, "—");
    }
}
