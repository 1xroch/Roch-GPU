//! The telemetry window's table (C# `ViewModels/TelemetryTable.cs`): which sensors exist for this
//! card, their running statistics, and the display rows. Toolkit-free; the monitor window draws
//! [`TelemetryTable::rows`].
//!
//! Rows are held flat rather than nested so the banding can count only what is visible — banding
//! by position breaks the moment a group is folded away, leaving two shaded rows adjacent.

use std::collections::{BTreeMap, HashMap, HashSet};

use roch_core::{GpuCapabilities, GpuTelemetry, SensorStat, VoltageControlStyle};

const DASH: &str = "—";

/// One line of the table: a group heading, or a sensor with its four figures.
#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryRow {
    pub name: String,
    pub is_header: bool,
    /// Alternating band, recomputed as rows are inserted.
    pub is_banded: bool,
    pub current: String,
    pub minimum: String,
    pub maximum: String,
    pub average: String,
    /// The statistic behind a sensor row (`None` for a heading).
    key: Option<String>,
    unit: String,
    decimals: usize,
}

impl TelemetryRow {
    pub(crate) fn sensor_key(&self) -> Option<&str> {
        self.key.as_deref()
    }
    /// A heading has no reading of its own, and an em dash in all four columns reads as four
    /// sensors that failed rather than as a title.
    fn header(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            is_header: true,
            is_banded: false,
            current: String::new(),
            minimum: String::new(),
            maximum: String::new(),
            average: String::new(),
            key: None,
            unit: String::new(),
            decimals: 0,
        }
    }

    fn sensor(name: &str, key: &str, unit: &str, decimals: usize) -> Self {
        Self {
            name: name.to_owned(),
            is_header: false,
            is_banded: false,
            current: DASH.into(),
            minimum: DASH.into(),
            maximum: DASH.into(),
            average: DASH.into(),
            key: Some(key.to_owned()),
            unit: unit.to_owned(),
            decimals,
        }
    }

    /// Re-read the statistic into the four display strings. A Yes/No row has no minimum or
    /// maximum worth showing, and its average is the share of the time it was on; a PCIe link row
    /// keeps its minimum and maximum (a link that drops speed at idle shows there) but not an average.
    fn refresh(&mut self, stats: &HashMap<String, SensorStat>) {
        let Some(s) = self.key.as_ref().and_then(|k| stats.get(k)) else { return };
        self.current = self.format(s.current);
        if self.unit == "state" {
            self.minimum = DASH.into();
            self.maximum = DASH.into();
            let a = s.average();
            self.average = if a.is_nan() { DASH.into() } else { format!("{:.0} % of the time", a * 100.0) };
            return;
        }
        self.minimum = self.format(s.minimum);
        self.maximum = self.format(s.maximum);
        self.average = if is_link_unit(&self.unit) { DASH.into() } else { self.format(s.average()) };
    }

    fn format(&self, v: f64) -> String {
        format_reading(v, &self.unit, self.decimals)
    }
}

/// C# `TelemetryRow.Format`: an em dash for NaN; a 0/1 "state" sensor as No/Yes, and its average
/// as the fraction of samples it was active; otherwise `F<decimals>` and the unit.
pub fn format_reading(v: f64, unit: &str, decimals: usize) -> String {
    if v.is_nan() {
        return DASH.into();
    }
    if unit == "state" {
        return if v == 0.0 {
            "No".into()
        } else if v == 1.0 {
            "Yes".into()
        } else {
            format!("{:.1} % active", v * 100.0)
        };
    }
    match unit {
        "gen" => format!("Gen {v:.0}"),
        "lanes" => format!("x{v:.0}"),
        _ => format!("{v:.decimals$} {unit}"),
    }
}

/// Names for the clock domains NVIDIA only numbers (C# `NvApiBackend.DomainName`; mirrors
/// `roch_nvidia`'s copy, kept here so the window does not depend on a backend crate's internals).
/// Checked against a 5070 Ti. A domain with no name gets no row: "Domain type 31" read a flat
/// 0 MHz and told the reader nothing.
pub fn domain_name(type_: i32) -> Option<&'static str> {
    match type_ {
        3 => Some("HUBCLK"),
        6 => Some("DISPCLK"),
        20 => Some("L2CLK"),
        22 => Some("Reference clock"),
        _ => None,
    }
}

/// What each power-monitor channel is, by position (C# `NvApiBackend.PowerRailName`, mirrored in
/// `roch_nvidia`). Positional, so only applied to a card reporting exactly this many channels;
/// anything else is numbered.
///
/// The three the comparison monitor called only "Rail 218/214/212" are named by how the readings
/// add up, which held to 0.01 A on a 5070 Ti in the current, minimum and average alike: the board
/// total is the PCIe slot plus 218, so 218 is the power connector; 214 is NVVDD input plus 212, so
/// 214 is the GPU's rails together and 212 the GPU rail other than the core's; and the board total
/// is also 214 plus Misc0.
pub fn power_rail_name(index: usize, total: usize) -> String {
    const KNOWN: [&str; 7] = [
        "Board total",
        "PCIe slot 12 V",
        "Power connector 12 V",
        "GPU rails (NVVDD + other)",
        "NVVDD input",
        "Other GPU rail input",
        "Misc0 input",
    ];
    if total == KNOWN.len() && index < KNOWN.len() { KNOWN[index].into() } else { format!("Rail {index}") }
}

/// The PCIe link figures read "Gen 5" and "x16"; their average says nothing, so it is not shown.
fn is_link_unit(unit: &str) -> bool {
    matches!(unit, "gen" | "lanes" | "GT/s")
}

/// The row that stands for every memory chip: the hottest of them, with which one it is.
pub const HOTTEST_CHIP_KEY: &str = "memchips-hottest";

/// One memory chip's own row (the NVIDIA backend's "Memory chip A0" sensors). The window shows
/// these only when asked to; [`HOTTEST_CHIP_KEY`] stands for them otherwise.
pub fn is_memory_chip_key(key: &str) -> bool {
    key.starts_with("nv:memchip-")
}

/// "Memory chip B2" → "B2" (and "B2 (back)" for a back-of-board chip).
fn chip_label(name: &str) -> String {
    name.strip_prefix("Memory chip ").unwrap_or(name).to_string()
}

/// The whole table. Built once from the first sample, then fed a sample at a time.
#[derive(Debug, Default)]
pub struct TelemetryTable {
    rows: Vec<TelemetryRow>,
    stats: HashMap<String, SensorStat>,
    supplemental_keys: HashSet<String>,
    /// Which chip is the hottest in the latest sample, for the [`HOTTEST_CHIP_KEY`] row.
    hottest_chip: Option<String>,
}

impl TelemetryTable {
    /// Lay out the sensors this card actually has. A row for a sensor the card never reports would
    /// sit at an em dash for the whole session, which reads as a fault rather than an absence.
    pub fn new<'a>(
        first: &GpuTelemetry,
        caps: &GpuCapabilities,
        extra_clock_keys: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let extra: Vec<&str> = extra_clock_keys.into_iter().collect();
        let mut t = Self::default();

        t.group("Temperatures");
        t.sensor("temp", "GPU", "°C", 1);
        if !first.hot_spot_c.is_nan() {
            t.sensor("hotspot", "Hot spot", "°C", 1);
        }
        if !first.memory_temperature_c.is_nan() {
            t.sensor("memtemp", "Memory junction", "°C", 1);
        }

        t.group("Voltages");
        if !first.voltage_mv.is_nan() {
            t.sensor("volt", "GPU core (VID)", "mV", 0);
        }
        if !first.nvvdd_mv.is_nan() {
            t.sensor("nvvdd", "NVVDD (measured)", "mV", 1);
        }
        if !first.msvdd_mv.is_nan() {
            t.sensor("msvdd", "MSVDD (measured)", "mV", 1);
        }

        t.group("Clocks");
        t.sensor("core", "GPU core", "MHz", 0);
        if !caps.power_limit_is_offset && caps.voltage_style != VoltageControlStyle::Percent {
            t.sensor("coremeasured", "GPU core (measured)", "MHz", 0);
        }
        // A card whose memory control is a data rate (Intel) reports in the editor's unit.
        if first.memory_speed_mbps.is_finite() {
            t.sensor("memspeed", "Memory", "Mbps", 0);
        } else {
            t.sensor("mem", "Memory", "MHz", 0);
        }
        if !first.fabric_clock_mhz.is_nan() {
            t.sensor("fclk", "Fabric (FCLK)", "MHz", 0);
        }
        if !first.soc_clock_mhz.is_nan() {
            t.sensor("socclk", "SoC", "MHz", 0);
        }
        if caps.can_set_xbar_offset || extra.contains(&"xbar") {
            t.sensor("xbar", "Crossbar", "MHz", 0);
        }
        if caps.can_set_sys_offset || extra.contains(&"sys") {
            t.sensor("sys", "SYS", "MHz", 0);
        }
        if caps.can_set_video_offset {
            t.sensor("video", "Video", "MHz", 0);
        }
        // Whatever else this card reports: the backend names what has been checked against the
        // hardware and numbers the rest. Built from the first sample, so a card that reports
        // nothing extra gets no rows rather than a column of dashes. In ascending domain type, the
        // order the C# backend inserted them in: the keys arrive sorted as text ("domain20" before
        // "domain3"), which put L2CLK above HUBCLK.
        let mut domains: Vec<(i32, &str)> = extra
            .iter()
            .filter_map(|key| key.strip_prefix("domain").and_then(|s| s.parse::<i32>().ok()).map(|n| (n, *key)))
            .collect();
        domains.sort_by_key(|&(n, _)| n);
        for (n, key) in domains {
            if let Some(name) = domain_name(n) {
                t.sensor(key, name, "MHz", 0);
            }
        }

        t.group("Load");
        t.sensor("load", "GPU core", "%", 0);
        t.sensor("memload", "Memory controller", "%", 0);

        t.group("Power");
        if first.power_watts > 0.0 || caps.voltage_style == VoltageControlStyle::Percent {
            t.sensor("watts", "Board draw", "W", 1);
        }
        if !caps.power_limit_is_offset || first.power_percent.is_finite() {
            t.sensor("tdp", "Total, % of TDP", "%", 1);
        }

        // Measured 12 V rail currents from the card's power monitor. Index 0 is the board total;
        // the rest are the supply rails behind it. NOT the current an OCP limit guards — those
        // rails sit after the VRM at about a volt, and this family does not report them.
        if !first.rail_amps.is_empty() {
            t.group("Rail current");
            let n = first.rail_amps.len();
            for i in 0..n {
                t.sensor(&format!("railA{i}"), &power_rail_name(i, n), "A", 2);
            }
        }

        t.group("Fans");
        let fans = first.fan_rpms.len().max(1);
        for i in 0..fans {
            let rpm = if fans == 1 { "Speed".to_owned() } else { format!("Fan {}", i + 1) };
            let pct = if fans == 1 { "Duty".to_owned() } else { format!("Fan {} duty", i + 1) };
            t.sensor(&format!("fanrpm{i}"), &rpm, "RPM", 0);
            t.sensor(&format!("fanpct{i}"), &pct, "%", 0);
        }

        t.group("Memory");
        t.sensor("memused", if caps.power_limit_is_offset { "Dedicated VRAM used" } else { "Allocated" }, "MB", 0);

        t.restripe();
        t
    }

    pub fn rows(&self) -> &[TelemetryRow] {
        &self.rows
    }

    /// Every sensor row, headings excluded (the audit dump's figure; the window counts what is visible).
    #[cfg(test)]
    pub fn sensor_count(&self) -> usize {
        self.rows.iter().filter(|r| !r.is_header).count()
    }

    fn stat(&mut self, key: &str) {
        self.stats.entry(key.to_owned()).or_default();
    }

    fn group(&mut self, name: &str) {
        self.rows.push(TelemetryRow::header(&name.to_uppercase()));
    }

    fn sensor(&mut self, key: &str, label: &str, unit: &str, decimals: usize) {
        self.stat(key);
        self.rows.push(TelemetryRow::sensor(label, key, unit, decimals));
    }

    /// Alternating bands on the data rows; titles carry their own tone.
    fn restripe(&mut self) {
        let mut data = 0;
        for r in &mut self.rows {
            if r.is_header {
                data = 0;
                continue;
            }
            r.is_banded = data % 2 == 1;
            data += 1;
        }
    }

    fn put(&mut self, key: &str, v: f64) {
        if let Some(s) = self.stats.get_mut(key) {
            s.add(v);
        }
    }

    /// Feed one sample in and refresh the display strings.
    pub fn add(&mut self, t: &GpuTelemetry, extra_clocks: Option<&BTreeMap<String, f64>>) {
        // Some sensors become available after the first poll (driver warmup or waking from idle).
        let discover = [
            ("hotspot", "Hot spot", "°C", t.hot_spot_c),
            ("memtemp", "Memory junction", "°C", t.memory_temperature_c),
            ("volt", "GPU core (VID)", "mV", t.voltage_mv),
            ("watts", "Board draw", "W", t.power_watts),
            ("fclk", "Fabric (FCLK)", "MHz", t.fabric_clock_mhz),
            ("socclk", "SoC", "MHz", t.soc_clock_mhz),
        ];
        for (key, name, unit, value) in discover {
            if value.is_finite() && !self.stats.contains_key(key) {
                self.insert_sensor(key, name, unit);
            }
        }

        self.put("temp", t.temperature_c);
        self.put("hotspot", t.hot_spot_c);
        self.put("memtemp", t.memory_temperature_c);
        self.put("volt", t.voltage_mv);
        self.put("nvvdd", t.nvvdd_mv);
        self.put("msvdd", t.msvdd_mv);
        self.put("core", t.core_clock_mhz);
        self.put("mem", t.memory_clock_mhz);
        self.put("memspeed", t.memory_speed_mbps);
        self.put("fclk", t.fabric_clock_mhz);
        self.put("socclk", t.soc_clock_mhz);
        self.put("load", t.gpu_load_percent);
        self.put("memload", t.memory_load_percent);
        self.put("watts", t.power_watts);
        for (i, a) in t.rail_amps.iter().enumerate() {
            self.put(&format!("railA{i}"), *a);
        }
        self.put("tdp", t.power_percent);
        self.put("memused", t.memory_used_mb);

        // The memory chips as one row, ahead of their own: the hottest, and which chip it is. Its
        // minimum, maximum and average follow that hottest figure over the session.
        let hottest = t
            .supplemental_sensors
            .iter()
            .filter(|s| is_memory_chip_key(&s.key) && s.value.is_finite())
            .max_by(|a, b| a.value.total_cmp(&b.value));
        if hottest.is_some() && !self.stats.contains_key(HOTTEST_CHIP_KEY) {
            self.insert_sensor(HOTTEST_CHIP_KEY, "Memory chips (hottest)", "°C");
        }
        self.hottest_chip = hottest.map(|h| chip_label(&h.name));
        self.put(HOTTEST_CHIP_KEY, hottest.map_or(f64::NAN, |h| h.value));

        for s in &t.supplemental_sensors {
            if self.supplemental_keys.insert(s.key.clone()) {
                self.insert_sensor(&s.key, &s.name, &s.unit);
            }
        }
        // A sensor that stopped reporting clears its current reading but keeps its history.
        let present: HashSet<&str> = t.supplemental_sensors.iter().map(|s| s.key.as_str()).collect();
        let missing: Vec<String> =
            self.supplemental_keys.iter().filter(|k| !present.contains(k.as_str())).cloned().collect();
        for k in missing {
            self.put(&k, f64::NAN);
        }
        for s in &t.supplemental_sensors {
            self.put(&s.key, s.value);
        }

        for (i, v) in t.fan_rpms.iter().enumerate() {
            self.put(&format!("fanrpm{i}"), *v);
        }
        for (i, v) in t.fan_percents.iter().enumerate() {
            self.put(&format!("fanpct{i}"), *v);
        }
        if t.fan_rpms.is_empty() {
            self.put("fanrpm0", t.fan_rpm);
            self.put("fanpct0", t.fan_percent);
        }

        if let Some(extra) = extra_clocks {
            for (k, v) in extra {
                self.put(k, *v);
            }
        }
        self.refresh();
    }

    fn refresh(&mut self) {
        let stats = &self.stats;
        for r in &mut self.rows {
            r.refresh(stats);
            if r.key.as_deref() == Some(HOTTEST_CHIP_KEY)
                && r.current != DASH
                && let Some(chip) = &self.hottest_chip
            {
                r.current = format!("{} · {chip}", r.current);
            }
        }
    }

    /// A sensor first seen after the table was built goes at the end of its group (created at
    /// the bottom when the card had none of that kind before).
    fn insert_sensor(&mut self, key: &str, name: &str, unit: &str) {
        let group = match (key, unit) {
            ("amdgpu:vram_usage" | "amdgpu:gtt_usage", "%") => "MEMORY",
            (_, "°C") => "TEMPERATURES",
            (_, "mV" | "V") => "VOLTAGES",
            (_, "MHz") => "CLOCKS",
            (_, "MB/s" | "GB/s") => "MEMORY BANDWIDTH",
            (_, "W") => "POWER",
            (_, "MB") => "MEMORY",
            (_, "gen" | "lanes" | "GT/s") => "PCIe LINK",
            (_, "%") => "LOAD",
            (_, "A") => "RAIL CURRENT",
            _ => "LIMITERS",
        }
        .to_uppercase();
        let heading = match self.rows.iter().position(|r| r.is_header && r.name == group) {
            Some(i) => i,
            None => {
                self.group(&group);
                self.rows.len() - 1
            }
        };
        let end =
            self.rows.iter().skip(heading + 1).position(|r| r.is_header).map_or(self.rows.len(), |i| heading + 1 + i);
        let decimals = match unit {
            "°C" | "W" | "%" | "GT/s" => 1,
            "V" => 3,
            "A" => 2,
            _ => 0,
        };
        self.stat(key);
        self.rows.insert(end, TelemetryRow::sensor(name, key, unit, decimals));
        self.restripe();
    }

    /// Clear minimum, maximum and average (Reset statistics). GPU settings are not touched.
    pub fn reset_stats(&mut self) {
        for s in self.stats.values_mut() {
            s.reset();
        }
        self.refresh();
    }
}

#[cfg(test)]
mod tests {
    //! The checks of `tests/GpuTuner.Telemetry.Tests/Program.cs`.
    use roch_core::SupplementalSensor;

    use super::*;

    fn empty() -> GpuTelemetry {
        GpuTelemetry {
            hot_spot_c: f64::NAN,
            memory_temperature_c: f64::NAN,
            voltage_mv: f64::NAN,
            power_watts: f64::NAN,
            power_percent: f64::NAN,
            ..Default::default()
        }
    }

    fn row<'a>(t: &'a TelemetryTable, name: &str) -> &'a TelemetryRow {
        let all: Vec<_> = t.rows().iter().filter(|r| r.name == name).collect();
        assert_eq!(all.len(), 1, "exactly one row named {name}");
        all[0]
    }

    fn group_of(t: &TelemetryTable, name: &str) -> String {
        let i = t.rows().iter().position(|r| r.name == name).expect("row exists");
        t.rows()[..i].iter().rev().find(|r| r.is_header).expect("has a heading").name.clone()
    }

    fn sup(key: &str, name: &str, unit: &str, v: f64) -> SupplementalSensor {
        SupplementalSensor::new(key, name, unit, v)
    }

    fn striped(t: &TelemetryTable) -> bool {
        let mut data = 0;
        let mut ok = true;
        for r in t.rows() {
            if r.is_header {
                data = 0;
            } else {
                ok &= r.is_banded == (data % 2 == 1);
                data += 1;
            }
        }
        ok
    }

    #[test]
    fn amd_style_table_discovers_groups_and_clears_missing_sensors() {
        let caps = GpuCapabilities { power_limit_is_offset: true, ..Default::default() };
        let mut table = TelemetryTable::new(&empty(), &caps, []);
        assert!(!table.rows().iter().any(|r| r.name == "Parameter"), "column labels are not repeated");
        assert!(!table.rows().iter().any(|r| r.name.contains("effective")), "no unsupported effective clock");
        assert!(
            !table.rows().iter().any(|r| r.name.contains("TDP") || r.name.contains("unavailable")),
            "no unavailable power utilisation placeholder"
        );

        let sample = GpuTelemetry {
            fabric_clock_mhz: 2401.0,
            soc_clock_mhz: 1371.0,
            power_watts: 34.0,
            memory_used_mb: 292.0,
            supplemental_sensors: vec![
                sup("adlx:shared", "Shared GPU memory used", "MB", 91.0),
                sup("adl:10", "Core VRM", "°C", 43.0),
                sup("adl:16", "SoC voltage", "mV", 940.0),
                sup("adl:41", "PCIe link width", "lanes", 16.0),
            ],
            ..empty()
        };
        table.add(&sample, None);
        assert_eq!(row(&table, "Fabric (FCLK)").current, "2401 MHz", "late FCLK discovery");
        assert_eq!(row(&table, "SoC").current, "1371 MHz", "late SoC discovery");
        assert_eq!(row(&table, "Dedicated VRAM used").current, "292 MB", "VRAM updates existing row");
        assert_eq!(group_of(&table, "Shared GPU memory used"), "MEMORY");
        assert_eq!(group_of(&table, "Core VRM"), "TEMPERATURES");
        assert_eq!(group_of(&table, "SoC voltage"), "VOLTAGES");
        assert_eq!(group_of(&table, "PCIe link width"), "PCIE LINK");

        let count = table.rows().len();
        table.add(&sample, None);
        assert_eq!(count, table.rows().len(), "no duplicate rows on repeated polls");

        table.add(&empty(), None);
        let shared = row(&table, "Shared GPU memory used");
        assert_eq!(shared.current, "—", "disconnected sensor current cleared");
        assert!(shared.minimum == "91 MB" && shared.maximum == "91 MB", "disconnected sensor history retained");
        assert_eq!(row(&table, "Fabric (FCLK)").current, "—", "disconnected late-discovered clock cleared");

        table.reset_stats();
        let shared = row(&table, "Shared GPU memory used");
        assert!(shared.average == "—" && shared.minimum == "—", "reset clears history");
        table.add(&sample, None);
        assert_eq!(row(&table, "Shared GPU memory used").current, "91 MB", "sensor can recover");
        assert!(striped(&table), "inserted sensors preserve striping");

        table.add(
            &GpuTelemetry {
                supplemental_sensors: vec![
                    sup("nv:video-load", "Video engine", "%", 0.0),
                    sup("nv:pcie-speed", "PCIe link speed", "GT/s", 2.5),
                    sup("nv:memory-free", "Available VRAM", "MB", 11000.0),
                    sup("nv:limit-power", "Power limit active", "state", 0.0),
                ],
                ..empty()
            },
            None,
        );
        assert_eq!(group_of(&table, "Video engine"), "LOAD");
        assert_eq!(group_of(&table, "PCIe link speed"), "PCIE LINK");
        assert_eq!(group_of(&table, "Available VRAM"), "MEMORY");
        assert_eq!(row(&table, "Power limit active").current, "No", "idle limit displays No");
        assert_eq!(row(&table, "PCIe link speed").current, "2.5 GT/s", "fractional PCIe speed retained");

        table.add(
            &GpuTelemetry {
                supplemental_sensors: vec![sup("nv:limit-power", "Power limit active", "state", 1.0)],
                ..empty()
            },
            None,
        );
        assert_eq!(row(&table, "Power limit active").current, "Yes", "active limit displays Yes");
        assert_eq!(row(&table, "Power limit active").average, "50 % of the time", "limit average is active share");
        assert_eq!(row(&table, "Power limit active").minimum, "—", "a Yes/No row has no minimum");
        assert_eq!(row(&table, "Power limit active").maximum, "—", "nor a maximum");
        assert_eq!(row(&table, "PCIe link speed").minimum, "2.5 GT/s", "a link keeps its minimum");
        assert_eq!(row(&table, "PCIe link speed").average, "—", "but has no average");
        assert_eq!(row(&table, "Video engine").current, "—", "missing video reading cleared");
        assert!(striped(&table));
    }

    #[test]
    fn read_only_clock_domains_and_measured_core() {
        let t = TelemetryTable::new(&empty(), &GpuCapabilities::default(), ["xbar", "sys"]);
        assert!(t.rows().iter().any(|r| r.name == "Crossbar"), "read-only XBAR telemetry visible");
        assert!(t.rows().iter().any(|r| r.name == "SYS"), "read-only SYS telemetry visible");
        assert!(
            t.rows().iter().any(|r| r.name == "GPU core (measured)")
                && !t.rows().iter().any(|r| r.name.contains("effective")),
            "instantaneous clock not called effective"
        );
    }

    #[test]
    fn intel_table() {
        let caps = GpuCapabilities { voltage_style: VoltageControlStyle::Percent, ..Default::default() };
        let mut t = TelemetryTable::new(&empty(), &caps, []);
        assert!(!t.rows().iter().any(|r| r.name == "GPU core (measured)"), "Intel has no invented measured core");
        assert!(t.rows().iter().any(|r| r.name == "Board draw"), "board-power row exists before second sample");
        t.add(&GpuTelemetry { power_watts: 36.5, ..empty() }, None);
        assert_eq!(row(&t, "Board draw").current, "36.5 W", "second sample populates board power");
        t.add(
            &GpuTelemetry {
                supplemental_sensors: vec![
                    sup("intel-memory-read", "Memory read bandwidth", "MB/s", 122818.0),
                    sup("intel-memory-voltage", "Memory voltage", "V", 1.352),
                    sup("intel-sa-vr", "SA voltage regulator", "°C", 71.0),
                    sup("intel-gpu-power", "GPU power", "W", 110.196),
                ],
                ..empty()
            },
            None,
        );
        assert!(t.rows().iter().any(|r| r.is_header && r.name == "MEMORY BANDWIDTH"), "bandwidth gets its own group");
        assert_eq!(row(&t, "Memory voltage").current, "1.352 V", "memory voltage retains precision");
        assert_eq!(row(&t, "GPU power").current, "110.2 W", "GPU power rendered");
    }

    #[test]
    fn amd_memory_occupancy_is_grouped_apart_from_engine_load() {
        let sample = GpuTelemetry {
            supplemental_sensors: vec![
                sup("amdgpu:vram_usage", "Dedicated VRAM occupancy", "%", 84.0),
                sup("amdgpu:gtt_usage", "Shared GPU memory occupancy", "%", 1.5),
                sup("amdgpu:video_load", "Video engine", "%", 0.0),
            ],
            ..empty()
        };
        let mut table = TelemetryTable::new(&sample, &GpuCapabilities::default(), []);
        table.add(&sample, None);
        assert_eq!(group_of(&table, "Dedicated VRAM occupancy"), "MEMORY");
        assert_eq!(group_of(&table, "Shared GPU memory occupancy"), "MEMORY");
        assert_eq!(group_of(&table, "Video engine"), "LOAD");
        assert_eq!(row(&table, "Video engine").current, "0.0 %");
    }

    #[test]
    fn memory_speed_replaces_the_memory_clock() {
        let first = GpuTelemetry { memory_clock_mhz: 2375.0, memory_speed_mbps: 19000.0, ..empty() };
        let mut t = TelemetryTable::new(&first, &GpuCapabilities::default(), []);
        t.add(&first, None);
        assert_eq!(row(&t, "Memory").current, "19000 Mbps", "Intel memory in the editor's unit");
        let mut t = TelemetryTable::new(&empty(), &GpuCapabilities::default(), []);
        t.add(&GpuTelemetry { memory_clock_mhz: 10501.0, ..empty() }, None);
        assert_eq!(row(&t, "Memory").current, "10501 MHz", "other cards keep the clock");
    }

    #[test]
    fn nvidia_extras_rails_and_fans() {
        let first = GpuTelemetry {
            rail_amps: vec![25.0, 0.5, 1.0, 1.0, 20.0, 1.0, 0.2],
            fan_rpms: vec![1000.0, 1010.0],
            fan_percents: vec![30.0, 31.0],
            hot_spot_c: f64::NAN,
            memory_temperature_c: 40.0,
            ..Default::default()
        };
        let extra: BTreeMap<String, f64> = [("domain20".to_owned(), 2227.0), ("xbar".to_owned(), 884.0)].into();
        let mut t = TelemetryTable::new(&first, &GpuCapabilities::default(), extra.keys().map(String::as_str));
        assert_eq!(group_of(&t, "L2CLK"), "CLOCKS");
        assert_eq!(group_of(&t, "NVVDD input"), "RAIL CURRENT");
        assert!(t.rows().iter().any(|r| r.name == "Fan 2 duty"));
        assert!(!t.rows().iter().any(|r| r.name == "Hot spot"), "a NaN sensor gets no row");
        t.add(&first, Some(&extra));
        assert_eq!(row(&t, "L2CLK").current, "2227 MHz");
        assert_eq!(row(&t, "Crossbar").current, "884 MHz");
        assert_eq!(row(&t, "Board total").current, "25.00 A");
        assert_eq!(row(&t, "Fan 2").current, "1010 RPM");
        assert_eq!(row(&t, "Memory junction").current, "40.0 °C");
        assert!(striped(&t));
    }

    /// `measure_extra_clocks` is a BTreeMap, so the keys arrive in text order; the rows must still
    /// follow the domain type, as the C# (whose backend inserted them in ascending type) showed.
    #[test]
    fn clock_domains_are_listed_by_type_not_by_key_text() {
        let extra: BTreeMap<String, f64> =
            ["coremeasured", "domain20", "domain22", "domain3", "domain31", "domain6", "sys", "video", "xbar"]
                .iter()
                .map(|k| (k.to_string(), 0.0))
                .collect();
        let t = TelemetryTable::new(&empty(), &GpuCapabilities::default(), extra.keys().map(String::as_str));
        let names: Vec<&str> = t.rows().iter().map(|r| r.name.as_str()).collect();
        let at = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(at("HUBCLK") < at("DISPCLK"));
        assert!(at("DISPCLK") < at("L2CLK"));
        assert!(at("L2CLK") < at("Reference clock"));
        assert!(!names.iter().any(|n| n.starts_with("Domain type")), "an unnamed domain gets no row");
        assert_eq!(at("SYS") + 1, at("HUBCLK"), "domains follow the named clocks");
    }

    #[test]
    fn single_fan_card_falls_back_to_the_summary_reading() {
        let mut t = TelemetryTable::new(&empty(), &GpuCapabilities::default(), []);
        t.add(&GpuTelemetry { fan_rpm: 1500.0, fan_percent: 42.0, ..empty() }, None);
        assert_eq!(row(&t, "Speed").current, "1500 RPM");
        assert_eq!(row(&t, "Duty").current, "42 %");
    }

    #[test]
    fn memory_chips_stand_as_one_hottest_row() {
        let chips = |a0: f64, b2: f64| GpuTelemetry {
            supplemental_sensors: vec![
                sup("nv:memchip-a0", "Memory chip A0", "°C", a0),
                sup("nv:memchip-b2", "Memory chip B2", "°C", b2),
            ],
            memory_temperature_c: 56.0,
            ..empty()
        };
        let mut t = TelemetryTable::new(&chips(54.0, 58.0), &GpuCapabilities::default(), []);
        t.add(&chips(54.0, 58.0), None);
        let names: Vec<&str> = t.rows().iter().map(|r| r.name.as_str()).collect();
        let at = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(at("Memory junction") < at("Memory chips (hottest)"));
        assert!(at("Memory chips (hottest)") < at("Memory chip A0"), "stands ahead of the chips it covers");
        assert_eq!(group_of(&t, "Memory chips (hottest)"), "TEMPERATURES");
        assert_eq!(row(&t, "Memory chips (hottest)").current, "58.0 °C · B2");
        // The hottest moves to another chip; the statistics follow the hottest figure.
        t.add(&chips(60.0, 57.0), None);
        let r = row(&t, "Memory chips (hottest)");
        assert_eq!(
            (r.current.as_str(), r.minimum.as_str(), r.maximum.as_str()),
            ("60.0 °C · A0", "58.0 °C", "60.0 °C")
        );
        assert!(is_memory_chip_key("nv:memchip-a0") && !is_memory_chip_key(HOTTEST_CHIP_KEY));
        // A card without chip sensors gets no such row.
        let plain = TelemetryTable::new(&empty(), &GpuCapabilities::default(), []);
        assert!(!plain.rows().iter().any(|r| r.name == "Memory chips (hottest)"));
    }

    #[test]
    fn pcie_link_reads_as_generation_and_width() {
        assert_eq!(format_reading(5.0, "gen", 0), "Gen 5");
        assert_eq!(format_reading(16.0, "lanes", 0), "x16");
        assert_eq!(format_reading(32.0, "GT/s", 1), "32.0 GT/s");
    }

    #[test]
    fn names_match_the_backend() {
        assert_eq!(domain_name(31), None);
        assert_eq!(domain_name(20), Some("L2CLK"));
        assert_eq!(power_rail_name(0, 7), "Board total");
        assert_eq!(power_rail_name(2, 7), "Power connector 12 V");
        assert_eq!(power_rail_name(3, 7), "GPU rails (NVVDD + other)");
        assert_eq!(power_rail_name(5, 7), "Other GPU rail input");
        #[cfg(windows)]
        for i in 0..7 {
            assert_eq!(power_rail_name(i, 7), roch_nvidia::power_rail_name(i as i32, 7), "same name as the backend");
        }
        assert_eq!(power_rail_name(0, 3), "Rail 0");
    }
}
