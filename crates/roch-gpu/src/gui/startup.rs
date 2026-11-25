//! Everything the WPF `App.OnStartup` did before the main window existed: the flags, backend
//! detection (with the "start simulated instead?" offer), the recovery journal — recovered BEFORE
//! any logon profile so a crashed trial can never be replayed at boot — and the logon apply.
//!
//! Flags: `--mock` (simulated GPU), `--minimized` (straight to the tray), `--apply-profile <name>`
//! (apply that confirmed startup profile, then stay resident for fan curves), `--exit` (apply and
//! quit, no window).

use std::sync::Arc;

use roch_core::services::{AppSettings, ProfileStore, TuningService};
use roch_core::{GpuBackend, only_notes};

use super::shared::{Shared, StartupTasks};
use crate::backends;

/// The window's own command-line flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags {
    pub mock: bool,
    /// The word after `--mock`, when there is one (`rtx50`: see `backends::mock_variant`).
    pub mock_variant: Option<String>,
    pub exit_after: bool,
    pub minimized: bool,
    pub apply_profile: Option<String>,
}

impl Flags {
    pub fn parse(args: &[String]) -> Flags {
        let has = |f: &str| args.iter().any(|a| a == f);
        let apply_profile = args.iter().position(|a| a == "--apply-profile").and_then(|i| args.get(i + 1)).cloned();
        let mock_variant = args
            .iter()
            .position(|a| a == "--mock")
            .and_then(|i| args.get(i + 1))
            .filter(|a| !a.starts_with("--"))
            .cloned();
        Flags {
            mock: has("--mock"),
            mock_variant,
            exit_after: has("--exit"),
            minimized: has("--minimized"),
            apply_profile,
        }
    }
}

/// How startup ended.
pub enum Started {
    /// No window: quit with this code.
    Exit(i32),
    /// Open the window (in the tray when `minimized`).
    Window { shared: Shared, minimized: bool },
}

/// Run the startup sequence. `offer_mock` asks the user whether to fall back to the simulated GPU
/// when no supported card initialised (a modal dialog in the real app).
pub fn start(
    flags: &Flags,
    store: Arc<ProfileStore>,
    settings: AppSettings,
    startup: Arc<dyn StartupTasks>,
    offer_mock: impl FnOnce(&str) -> bool,
) -> Started {
    let log_path = store.root_directory().join("roch-gpu.log");
    let log = |s: &str| super::shared::log_line(&log_path, s);
    let mock = flags.mock || settings.use_mock_backend;
    let mut minimized = flags.minimized || settings.start_minimized;
    let mut apply_profile = flags.apply_profile.clone();
    let history = settings.history_seconds.max(1) as usize;
    let wanted_gpu = settings.gpu_index;

    // NVIDIA, AMD or Intel, whichever initialises. One binary, any card.
    let mut backend: Box<dyn GpuBackend> = backends::mock_variant(flags.mock_variant.as_deref());
    let mut detect_error = None;
    if !mock {
        match backends::create_and_initialize() {
            Ok((b, attempts)) => {
                for a in attempts {
                    log(&format!("Backend probe — {}: {}", a.name, a.error));
                }
                backend = b;
            }
            Err(e) => detect_error = Some(e),
        }
    }

    let make_service = |b: Box<dyn GpuBackend>| {
        let svc = Arc::new(TuningService::with_history(b, history));
        let path = log_path.clone();
        svc.add_log_listener(move |line| super::shared::log_line(&path, line));
        svc
    };
    let mut service = make_service(backend);
    let init = match detect_error {
        Some(e) => Err(e),
        None => service.initialize_preferred(wanted_gpu),
    };
    if let Err(e) = init {
        log(&format!("Init failed: {}", e.message));
        if mock || !offer_mock(&e.message) {
            return Started::Exit(1);
        }
        drop(service);
        service = make_service(backends::mock_variant(flags.mock_variant.as_deref()));
        if let Err(e) = service.initialize_preferred(wanted_gpu) {
            log(&format!("Init failed: {}", e.message));
            return Started::Exit(1);
        }
    }
    // A missing saved index (card removed, or a driver that reordered) is not fatal: the service
    // stays on the first card rather than refusing to start.
    let count = service.devices().len();
    if wanted_gpu >= count {
        log(&format!("Saved GPU {} is not present ({count} found); using the first.", wanted_gpu + 1));
    }

    let mut shared = Shared::new(service.clone(), store, settings, startup);

    // Recover BEFORE processing a logon profile so a crashed trial cannot be replayed at boot. A
    // logon apply then goes to every card, each through its own journal: a machine with two cards
    // tuned alike wants both tuned at logon, and each card's approval is its own. The selected card
    // goes last, so the journal the window keeps is that card's.
    let selected = service.gpu_index();
    let cards: Vec<usize> = match &apply_profile {
        Some(_) => (0..count).filter(|index| *index != selected).chain(std::iter::once(selected)).collect(),
        None => vec![selected],
    };
    let mut notices: Vec<String> = Vec::new();
    for index in cards {
        if index != service.gpu_index()
            && let Err(e) = service.select_gpu(index)
        {
            log(&format!("Could not select GPU {} for the startup profile: {}", index + 1, e.message));
            continue;
        }
        let label = if count > 1 {
            let device = service.device();
            format!(" on {} ({})", device.name, device.bus_id)
        } else {
            String::new()
        };
        let outcome = recover_and_apply(&shared, apply_profile.as_deref(), &label, count > 1, &log);
        if outcome.recovered && index == selected {
            apply_profile = None;
        }
        notices.extend(outcome.notice);
    }
    if !notices.is_empty() {
        minimized = false;
    }
    let notice = (!notices.is_empty()).then(|| notices.join("\n"));

    if flags.exit_after {
        // A fan curve cannot survive without a resident process; the startup docs say so.
        return Started::Exit(0);
    }
    // A recovery problem matters more; otherwise say what is keeping this card's controls away.
    let gpu = service.gpu_index();
    let notes = service.with_backend(|b| b.notes(gpu));
    for note in &notes {
        log(note);
    }
    shared.recovery_notice = notice.or_else(|| notes.first().cloned());
    Started::Window { shared, minimized }
}

/// What recovering one card, and applying the logon profile to it, came to.
struct CardOutcome {
    /// An interrupted trial was found (or the journal could not be read): nothing was applied.
    recovered: bool,
    notice: Option<String>,
}

/// Recover the selected card's journal, then apply the confirmed startup profile `name` to it.
/// `label` names the card in messages on a machine with more than one.
fn recover_and_apply(
    shared: &Shared,
    name: Option<&str>,
    label: &str,
    several: bool,
    log: &dyn Fn(&str),
) -> CardOutcome {
    let mut notice = None;
    let mut recovered = false;
    match shared.open_recovery() {
        Ok(mut recovery) => {
            if recovery.has_pending() {
                recovered = true;
                let errors = recovery.revert();
                let text = if errors.is_empty() || only_notes(&errors) {
                    format!("Recovered interrupted profile trial{label}. Startup apply was skipped.")
                } else {
                    format!("Profile recovery failed{label}: {}", errors.join("; "))
                };
                log(&text);
                notice = Some(text);
            }
            shared.set_recovery(Some(recovery));
        }
        Err(e) => {
            let text = format!("Profile recovery unavailable{label}: {}", e.message);
            log(&text);
            notice = Some(text);
            recovered = true;
        }
    }
    let Some(name) = name.filter(|_| !recovered) else {
        return CardOutcome { recovered, notice };
    };
    let confirmed = shared.recovery().as_ref().and_then(|r| r.startup_profile(name));
    let Some(profile) = confirmed else {
        let how = if several {
            format!(
                " Select that card and tick Startup, or run `rochgpu startup --enable \"{name}\"` to confirm it on every card."
            )
        } else {
            String::new()
        };
        let text = format!("Confirm '{name}' using the Startup checkbox before it can run at logon{label}.{how}");
        log(&text);
        return CardOutcome { recovered, notice: Some(text) };
    };
    let mut guard = shared.recovery();
    let Some(recovery) = guard.as_mut() else {
        return CardOutcome { recovered, notice };
    };
    match recovery.begin(&profile) {
        Ok(errs) if errs.is_empty() || only_notes(&errs) => match recovery.confirm() {
            Ok(()) => log(&format!("Confirmed startup profile '{name}' applied{label}.")),
            Err(e) => {
                let text = format!("Startup recovery interrupted{label}: {}", e.message);
                log(&text);
                notice = Some(text);
            }
        },
        Ok(errs) => {
            let rollback = recovery.revert();
            let tail = if recovery.has_pending() {
                format!(" Recovery failed: {}", rollback.join("; "))
            } else {
                " — restored fallback profile.".to_string()
            };
            let text = format!("Startup apply failed{label}: {}{tail}", errs.join("; "));
            log(&text);
            notice = Some(text);
        }
        Err(e) => {
            let text = format!("Startup recovery interrupted{label}: {}", e.message);
            log(&text);
            notice = Some(text);
        }
    }
    CardOutcome { recovered, notice }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    struct NoTask;

    impl StartupTasks for NoTask {
        fn exists(&self) -> bool {
            true
        }
        fn register(&self, _exe: &str, _profile: &str) -> roch_core::Result<()> {
            Ok(())
        }
        fn unregister(&self) -> roch_core::Result<()> {
            Ok(())
        }
    }

    /// Approve `name` for startup on card `index` of a simulated two-card machine, the way ticking
    /// Startup does: a confirmed trial, then the approval.
    fn approve(store: &ProfileStore, index: usize, name: &str) {
        let service = Arc::new(TuningService::new(backends::mock_variant(Some(backends::MOCK_MULTI))));
        service.initialize(index).unwrap();
        let mut recovery =
            roch_core::services::ProfileRecovery::for_selected(store.root_directory(), &service).unwrap();
        let mut profile = roch_core::TuningProfile { power_limit_percent: 90, ..Default::default() };
        profile.clamp_to(&service.capabilities());
        let messages = recovery.trial(&profile);
        assert!(messages.is_empty() || only_notes(&messages), "{messages:?}");
        recovery.approve_startup(name).unwrap();
    }

    fn logon(store: &Arc<ProfileStore>) -> String {
        let flags = Flags::parse(&v(&["--mock", "multi", "--apply-profile", "Slot 1", "--exit"]));
        let started = start(&flags, store.clone(), AppSettings::default(), Arc::new(NoTask), |_| false);
        assert!(matches!(started, Started::Exit(0)));
        std::fs::read_to_string(store.root_directory().join("roch-gpu.log")).unwrap()
    }

    #[test]
    fn a_logon_apply_reaches_every_card_that_approved_it() {
        let dir = std::env::temp_dir().join(format!("roch-startup-multi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Arc::new(ProfileStore::new(Some(dir.clone())).unwrap());

        // Approved on the second card only: that card is tuned, the first is told how to join it.
        approve(&store, 1, "Slot 1");
        let log = logon(&store);
        assert_eq!(log.matches("Confirmed startup profile 'Slot 1' applied on").count(), 1, "{log}");
        assert!(log.contains("PCI bus 2, slot 0).") || log.contains("(PCI bus 2, slot 0)"), "{log}");
        assert!(log.contains("before it can run at logon on"), "{log}");
        assert!(log.contains("rochgpu startup --enable"), "{log}");

        // Approved on both: both are tuned, and nothing is left to confirm.
        approve(&store, 0, "Slot 1");
        let _ = std::fs::remove_file(store.root_directory().join("roch-gpu.log"));
        let log = logon(&store);
        assert_eq!(log.matches("Confirmed startup profile 'Slot 1' applied on").count(), 2, "{log}");
        assert!(!log.contains("before it can run at logon"), "{log}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn flags_parse_like_the_wpf_app() {
        let f = Flags::parse(&v(&["--apply-profile", "Slot 1", "--minimized"]));
        assert_eq!(f.apply_profile.as_deref(), Some("Slot 1"));
        assert!(f.minimized && !f.mock && !f.exit_after);
        let f = Flags::parse(&v(&["--mock", "--exit", "--apply-profile"]));
        assert!(f.mock && f.exit_after);
        assert_eq!(f.mock_variant, None);
        let f = Flags::parse(&v(&["--mock", "rtx50", "--minimized"]));
        assert!(f.mock && f.minimized);
        assert_eq!(f.mock_variant.as_deref(), Some("rtx50"));
        assert_eq!(f.apply_profile, None);
    }
}
