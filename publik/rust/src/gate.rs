// SPDX-License-Identifier: GPL-3.0-or-later
//! "Sign in with publik": the window in front of the publik edition until the
//! person is signed in (publik's gate, as its Rust module has it).
//!
//! [`ensure_signed_in`] is the gate itself, written against [`GateUi`] so it
//! runs the same with the real window and in tests. [`hold`] wires it into
//! the app: at `on_ready` it hides the Hub, opens the gate window and runs
//! the gate on its own thread; once signed in it closes the window, brings
//! the Hub back as it was and runs the rest of `on_ready`. Closing the gate
//! window quits the app. Like publik's module there is no way past the gate
//! without publik: no account, no app.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

use super::client::{self, Env, Wallet, UNREACHABLE};
use super::USES_AI;

// ---------------------------------------------------------------------------
// Screens (publik's words, unchanged)

pub const SIGN_IN_TITLE: &str = "Sign in with publik";
pub const SIGN_IN_BODY: &str = "This app uses your publik account. Click Sign in with publik, then sign in in your browser. This window continues when you are signed in.";
pub const SIGN_IN_WAITING: &str = "Waiting for you to sign in in your browser.";
pub const SIGN_IN_TIMED_OUT: &str =
    "No sign-in after 10 minutes. Click Sign in with publik to try again.";
pub const PLAN_TITLE: &str = "Choose how to pay for AI";
pub const PLAN_WAITING: &str = "Waiting for your plan. Finish in your browser.";
pub const PLAN_TIMED_OUT: &str = "No plan after 10 minutes. Click Get a plan to try again.";
pub const CONNECT_TITLE: &str = "Connect your plan";

/// One screen of the gate. Buttons are `(id, label)`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Screen {
    pub id: &'static str,
    pub title: String,
    pub body: String,
    pub note: Option<String>,
    pub buttons: Vec<(&'static str, &'static str)>,
}

/// What shows the gate: the window in the app, a script in tests.
pub trait GateUi {
    /// Shows `screen` until a button is pressed (its id) or `done` turns
    /// true (`None`).
    fn show(&mut self, screen: &Screen, done: Arc<AtomicBool>) -> Option<&'static str>;
    /// Opens a publik page in the person's browser.
    fn open_url(&mut self, url: &str);
}

pub fn sign_in_screen(note: Option<&str>) -> Screen {
    Screen {
        id: "sign_in",
        title: SIGN_IN_TITLE.into(),
        body: SIGN_IN_BODY.into(),
        note: note.map(str::to_string),
        buttons: vec![("sign_in", "Sign in with publik")],
    }
}

pub fn unreachable_screen(message: &str) -> Screen {
    Screen {
        id: "unreachable",
        title: SIGN_IN_TITLE.into(),
        body: if message.is_empty() {
            UNREACHABLE.into()
        } else {
            message.into()
        },
        note: None,
        buttons: vec![("retry", "Retry")],
    }
}

fn who(wallet: &Value) -> String {
    match wallet
        .pointer("/account/handle")
        .and_then(Value::as_str)
        .filter(|h| !h.is_empty())
    {
        Some(handle) => format!("You are signed in as @{handle}."),
        None => "You are signed in to publik.".into(),
    }
}

fn what(wallet: &Value) -> String {
    match wallet
        .pointer("/plan/label")
        .and_then(Value::as_str)
        .filter(|l| !l.is_empty())
    {
        Some(label) => format!("Your plan: {label}."),
        None => {
            let micros = wallet
                .get("balance_micros")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            format!("Your balance: ${:.2}.", micros as f64 / 1_000_000.0)
        }
    }
}

pub fn choose_screen(wallet: &Value, note: Option<&str>) -> Screen {
    if client::has_plan(wallet) {
        Screen {
            id: "choose",
            title: CONNECT_TITLE.into(),
            body: format!(
                "{} {} Connect it to use AI in this app, or use your own key.",
                who(wallet),
                what(wallet)
            ),
            note: note.map(str::to_string),
            buttons: vec![
                ("connect", "Connect your plan"),
                ("own_key", "Use my own key"),
            ],
        }
    } else {
        Screen {
            id: "choose",
            title: PLAN_TITLE.into(),
            body: format!(
                "{} This app uses AI. Get a publik plan, or use your own key.",
                who(wallet)
            ),
            note: note.map(str::to_string),
            buttons: vec![("get_plan", "Get a plan"), ("own_key", "Use my own key")],
        }
    }
}

/// Where "Sign in with publik" sends the person.
fn claim_url(env: &Env, wallet: &Value) -> String {
    let from = |v: &Value| {
        v.get("claim_url")
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty())
            .map(str::to_string)
    };
    from(wallet)
        .or_else(|| from(&env.read_credential()))
        .unwrap_or_else(|| format!("{}/wallet", env.origin))
}

fn top_up_url(env: &Env, wallet: &Value) -> String {
    wallet
        .get("top_up_url")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}/wallet", env.origin))
}

// ---------------------------------------------------------------------------
// The gate

/// What the gate hands back once the person is through.
#[derive(Debug)]
pub struct Passed {
    pub key: String,
    pub wallet: Value,
    pub choice: Option<String>,
}

/// Looks at the wallet every `env.poll_every`, at most `env.polls` times,
/// until `check` holds; then sets `done`.
struct Poller {
    done: Arc<AtomicBool>,
    hit: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Poller {
    fn start(env: &Env, key: &str, check: fn(&Value) -> bool) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let hit = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (env, key) = (env.clone(), key.to_string());
        let (d, h, s) = (done.clone(), hit.clone(), stop.clone());
        std::thread::spawn(move || {
            for _ in 0..env.polls {
                std::thread::sleep(env.poll_every);
                if s.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(Wallet::Ok(wallet)) = client::read_wallet(&env, &key) {
                    if check(&wallet) {
                        h.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
            if !s.load(Ordering::SeqCst) {
                d.store(true, Ordering::SeqCst);
            }
        });
        Poller { done, hit, stop }
    }

    fn cancel(self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

enum Waited {
    /// `check` held.
    Done,
    /// A button other than the one that opens the browser.
    Pressed(&'static str),
}

/// Shows a screen whose `opener` button sends the person to `url`, and waits
/// for `check` to hold in the wallet: from the start, and again for ten
/// minutes after each press.
#[allow(clippy::too_many_arguments)]
fn wait_in_browser(
    ui: &mut dyn GateUi,
    env: &Env,
    key: &str,
    url: &str,
    screen: &dyn Fn(Option<&str>) -> Screen,
    opener: &str,
    waiting: &'static str,
    timed_out: &'static str,
    check: fn(&Value) -> bool,
) -> Waited {
    let mut note: Option<&'static str> = None;
    let mut poller = Some(Poller::start(env, key, check));
    loop {
        let done = poller
            .as_ref()
            .map(|p| p.done.clone())
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        match ui.show(&screen(note), done) {
            None => {
                let Some(p) = poller.take() else { continue };
                if p.hit.load(Ordering::SeqCst) {
                    return Waited::Done;
                }
                note = Some(timed_out);
            }
            Some(id) if id == opener => {
                ui.open_url(url);
                if let Some(p) = poller.take() {
                    p.cancel();
                }
                poller = Some(Poller::start(env, key, check));
                note = Some(waiting);
            }
            Some(id) => {
                if let Some(p) = poller.take() {
                    p.cancel();
                }
                return Waited::Pressed(id);
            }
        }
    }
}

/// Shows `screen` until a button is pressed.
fn ask(ui: &mut dyn GateUi, screen: &Screen) -> &'static str {
    loop {
        if let Some(id) = ui.show(screen, Arc::new(AtomicBool::new(false))) {
            return id;
        }
    }
}

/// publik's gate: a key for this computer (provisioning one if needed), a
/// signed-in publik account, and, for apps that use AI, the person's choice
/// of a publik plan or their own key. Returns only when all are there.
/// Usage counting starts after it ([`client::start_telemetry`]).
pub fn ensure_signed_in(env: &Env, ui: &mut dyn GateUi) -> Passed {
    let mut wiped = false;
    loop {
        let key = match env.key().map(Ok).unwrap_or_else(|| client::provision(env)) {
            Ok(key) => key,
            Err(message) => {
                ask(ui, &unreachable_screen(&message));
                continue;
            }
        };
        let wallet = match client::read_wallet(env, &key) {
            Ok(Wallet::Ok(wallet)) => wallet,
            // A stored key publik no longer knows: start this computer over.
            Ok(Wallet::Refused(401, _)) if env.api_key.is_none() && !wiped => {
                wiped = true;
                env.write_credential(&json!({}));
                continue;
            }
            Ok(Wallet::Refused(_, message)) => {
                ask(
                    ui,
                    &unreachable_screen(message.as_deref().unwrap_or(UNREACHABLE)),
                );
                continue;
            }
            Err(message) => {
                ask(ui, &unreachable_screen(&message));
                continue;
            }
        };

        if !client::is_claimed(&wallet) {
            let url = claim_url(env, &wallet);
            wait_in_browser(
                ui,
                env,
                &key,
                &url,
                &sign_in_screen,
                "sign_in",
                SIGN_IN_WAITING,
                SIGN_IN_TIMED_OUT,
                client::is_claimed,
            );
            continue;
        }

        let mut choice = env.choice();
        if USES_AI && choice.is_none() {
            if client::has_plan(&wallet) {
                match ask(ui, &choose_screen(&wallet, None)) {
                    "connect" => choice = Some("publik".into()),
                    "own_key" => choice = Some("own_key".into()),
                    _ => continue,
                }
                env.set_choice(choice.as_deref());
            } else {
                let url = top_up_url(env, &wallet);
                let screen = |note: Option<&str>| choose_screen(&wallet, note);
                if let Waited::Pressed("own_key") = wait_in_browser(
                    ui,
                    env,
                    &key,
                    &url,
                    &screen,
                    "get_plan",
                    PLAN_WAITING,
                    PLAN_TIMED_OUT,
                    client::has_plan,
                ) {
                    env.set_choice(Some("own_key"));
                }
                // With a plan now, the next round asks to connect it.
                continue;
            }
        }
        return Passed {
            key,
            wallet,
            choice,
        };
    }
}

// ---------------------------------------------------------------------------
// The window

/// The gate page as Vite emits it (`src/fork/vite-inputs.ts`).
pub const PAGE: &str = "publik/app/index.html";

static STARTED: AtomicBool = AtomicBool::new(false);
static PASSED: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct Shared {
    screen: Mutex<Option<Screen>>,
    presses: Mutex<Option<mpsc::Sender<String>>>,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(Shared::default)
}

/// The screen the gate window shows now (the page asks for it).
#[tauri::command]
fn publik_gate_screen() -> Option<Screen> {
    shared().screen.lock().ok().and_then(|s| s.clone())
}

/// A button on the gate window was pressed.
#[tauri::command]
fn publik_gate_press(id: String) {
    if let Some(tx) = shared().presses.lock().ok().and_then(|t| t.clone()) {
        let _ = tx.send(id);
    }
}

/// `true` for the gate window's commands (routed by the app's `fork::compose_invoke`).
pub fn handles(command: &str) -> bool {
    matches!(command, "publik_gate_screen" | "publik_gate_press")
}

pub fn invoke_handler<R: Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static
{
    tauri::generate_handler![publik_gate_screen, publik_gate_press]
}

/// The app's own window: while the gate stands it shows the sign-in page
/// instead of the Hub, so opening the app (Dock, menu bar, a second launch)
/// lands on "Sign in with publik" and never flashes the Hub.
const MAIN: &str = "main";

fn hub_url() -> &'static Mutex<Option<tauri::Url>> {
    static HUB: OnceLock<Mutex<Option<tauri::Url>>> = OnceLock::new();
    HUB.get_or_init(|| Mutex::new(None))
}

fn gate_url(hub: &tauri::Url) -> Option<tauri::Url> {
    hub.join(&format!("/{PAGE}")).ok()
}

fn is_gate_page(url: &tauri::Url) -> bool {
    url.path().trim_start_matches('/') == PAGE
}

/// Puts the sign-in page in the app's window (again, if the Hub was rebuilt
/// meanwhile). `show` also brings the window forward.
fn gate_in_main<R: Runtime>(app: &AppHandle<R>, show: bool) {
    let Some(main) = app.get_webview_window(MAIN) else {
        return;
    };
    // Handy creates this window titled "Handy" and the app renames it in
    // `on_ready`, which waits for the sign-in: name it now (Dock menu,
    // Window menu, Mission Control).
    let _ = main.set_title(&app.package_info().name);
    if let Ok(now) = main.url() {
        if !is_gate_page(&now) {
            let hub = {
                let mut saved = hub_url().lock().ok();
                if let Some(saved) = saved.as_deref_mut() {
                    if saved.is_none() {
                        *saved = Some(now.clone());
                    }
                }
                saved.and_then(|s| s.clone()).unwrap_or(now)
            };
            if let Some(gate) = gate_url(&hub) {
                if let Err(e) = main.navigate(gate) {
                    log::error!("publik: could not show the sign-in page: {e}");
                }
            }
        }
    }
    if show {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
    }
}

struct WindowUi<R: Runtime> {
    app: AppHandle<R>,
    presses: mpsc::Receiver<String>,
    /// The sign-in page is put up by the first screen it has to show, so a
    /// computer that is already signed in goes straight to the app.
    opened: bool,
}

impl<R: Runtime> GateUi for WindowUi<R> {
    fn show(&mut self, screen: &Screen, done: Arc<AtomicBool>) -> Option<&'static str> {
        if let Ok(mut current) = shared().screen.lock() {
            *current = Some(screen.clone());
        }
        if !self.opened {
            self.opened = true;
            // From the gate's thread, after `on_ready` has returned: window
            // work inside the Ready event deadlocks on Tauri's webview
            // manager lock (the app hung at launch with no window).
            gate_in_main(&self.app, true);
            log::info!("publik: sign-in page shown");
        }
        // A press meant for an earlier screen does not count.
        while self.presses.try_recv().is_ok() {}
        let mut ticks = 0u32;
        loop {
            if done.load(Ordering::SeqCst) {
                return None;
            }
            match self.presses.recv_timeout(Duration::from_millis(100)) {
                Ok(id) => {
                    if let Some((id, _)) = screen.buttons.iter().find(|(b, _)| *b == id) {
                        return Some(id);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
            ticks = (ticks + 1) % 5;
            if ticks == 0 {
                gate_in_main(&self.app, false);
            }
        }
    }

    fn open_url(&mut self, url: &str) {
        use tauri_plugin_opener::OpenerExt;
        if let Err(e) = self.app.opener().open_url(url, None::<&str>) {
            log::warn!("publik: could not open the browser: {e}");
        }
    }
}

/// Called by the app's `fork::on_ready` (publik edition only). `true` while
/// the gate stands: `on_ready` stops there, and `resume` (`on_ready` itself)
/// runs once the person is signed in.
pub fn hold<R: Runtime>(app: &AppHandle<R>, resume: fn(&AppHandle<R>)) -> bool {
    if PASSED.load(Ordering::SeqCst) {
        return false;
    }
    if STARTED.swap(true, Ordering::SeqCst) {
        return true;
    }
    let main = app.get_webview_window(MAIN);
    let main_was_visible = main
        .as_ref()
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    if let Some(main) = &main {
        let _ = main.hide();
    }
    let (tx, rx) = mpsc::channel();
    if let Ok(mut presses) = shared().presses.lock() {
        *presses = Some(tx);
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let mut ui = WindowUi {
            app: app.clone(),
            presses: rx,
            opened: false,
        };
        let passed = ensure_signed_in(&Env::from_process(), &mut ui);
        log::info!(
            "publik: signed in (choice {:?})",
            passed.choice.as_deref().unwrap_or("none")
        );
        // Someone who just signed in lands in the app, in front.
        let show_main = main_was_visible || ui.opened;
        PASSED.store(true, Ordering::SeqCst);
        client::start_telemetry();
        let hub = hub_url().lock().ok().and_then(|h| h.clone());
        let main_app = app.clone();
        let _ = app.run_on_main_thread(move || {
            resume(&main_app);
            if let Some(main) = main_app.get_webview_window(MAIN) {
                // The same window now loads the app.
                if let (Some(hub), Ok(now)) = (hub, main.url()) {
                    if is_gate_page(&now) {
                        let _ = main.navigate(hub);
                    }
                }
                let _ = main.set_title(&main_app.package_info().name);
                if show_main {
                    let _ = main.unminimize();
                    let _ = main.show();
                    let _ = main.set_focus();
                }
            }
        });
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{self, FakePublik};
    use std::collections::VecDeque;

    /// Presses buttons from a script; waits out `done` when told to.
    struct Script {
        steps: VecDeque<Option<&'static str>>,
        seen: Vec<Screen>,
        opened: Vec<String>,
    }

    impl Script {
        fn new(steps: &[Option<&'static str>]) -> Self {
            Script {
                steps: steps.iter().copied().collect(),
                seen: vec![],
                opened: vec![],
            }
        }
    }

    impl GateUi for Script {
        fn show(&mut self, screen: &Screen, done: Arc<AtomicBool>) -> Option<&'static str> {
            self.seen.push(screen.clone());
            match self
                .steps
                .pop_front()
                .expect("the gate asked for more than the script has")
            {
                Some(id) => Some(id),
                None => {
                    let start = std::time::Instant::now();
                    while !done.load(Ordering::SeqCst) {
                        assert!(start.elapsed() < Duration::from_secs(10), "never done");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    None
                }
            }
        }
        fn open_url(&mut self, url: &str) {
            self.opened.push(url.to_string());
        }
    }

    #[test]
    fn a_new_computer_signs_in_through_the_browser() {
        let publik = FakePublik::start();
        let env = fake::env(&publik);
        // Signed in on publik's site after the person clicks.
        publik.claim_after_wallet_reads(3);
        let mut ui = Script::new(&[Some("sign_in"), None]);
        let passed = ensure_signed_in(&env, &mut ui);
        assert!(passed.key.starts_with("pk_"));
        assert!(client::is_claimed(&passed.wallet));
        assert_eq!(ui.seen[0], sign_in_screen(None));
        assert_eq!(ui.seen[1], sign_in_screen(Some(SIGN_IN_WAITING)));
        assert_eq!(ui.opened, vec![format!("{}/claim/abc", publik.origin())]);
        // The key is kept for the next launch.
        assert_eq!(env.key().as_deref(), Some(passed.key.as_str()));
        assert_eq!(publik.installs(), 1);
    }

    #[test]
    fn a_signed_in_computer_goes_straight_through() {
        let publik = FakePublik::start();
        let env = fake::env(&publik);
        publik.claim_after_wallet_reads(0);
        let mut ui = Script::new(&[]);
        ensure_signed_in(&env, &mut ui);
        ensure_signed_in(&env, &mut ui);
        assert!(ui.seen.is_empty());
        assert_eq!(publik.installs(), 1);
    }

    #[test]
    fn no_sign_in_for_ten_minutes_says_so() {
        let publik = FakePublik::start();
        let env = fake::env(&publik);
        publik.claim_after_wallet_reads(u32::MAX);
        let mut ui = Script::new(&[None]);
        let stop = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ensure_signed_in(&env, &mut ui)
        }));
        assert!(stop.is_err(), "the script ran out, as planned");
        assert_eq!(
            ui.seen.last(),
            Some(&sign_in_screen(Some(SIGN_IN_TIMED_OUT)))
        );
    }

    #[test]
    fn publik_unreachable_offers_retry() {
        let publik = FakePublik::start();
        let mut env = fake::env(&publik);
        env.origin = "http://127.0.0.1:9".into();
        let mut ui = Script::new(&[Some("retry")]);
        let stop = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ensure_signed_in(&env, &mut ui)
        }));
        assert!(stop.is_err());
        assert_eq!(ui.seen[0], unreachable_screen(UNREACHABLE));
        assert_eq!(ui.seen.len(), 2);
    }

    #[test]
    fn a_build_without_a_token_says_so() {
        let publik = FakePublik::start();
        let mut env = fake::env(&publik);
        env.token = None;
        let mut ui = Script::new(&[Some("retry")]);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ensure_signed_in(&env, &mut ui)
        }));
        assert_eq!(ui.seen[0].body, client::NO_TOKEN);
    }

    #[test]
    fn a_forgotten_key_starts_the_computer_over() {
        let publik = FakePublik::start();
        let env = fake::env(&publik);
        env.write_credential(&json!({"key": "pk_old", "install_id": "x"}));
        publik.claim_after_wallet_reads(0);
        let passed = ensure_signed_in(&env, &mut Script::new(&[]));
        assert_ne!(passed.key, "pk_old");
        assert_eq!(publik.installs(), 1);
    }

    #[test]
    fn the_choose_screens_use_publiks_words() {
        let none = json!({"claim_state": "claimed", "account": {"handle": "finn"}});
        let s = choose_screen(&none, None);
        assert_eq!(s.title, PLAN_TITLE);
        assert_eq!(
            s.body,
            "You are signed in as @finn. This app uses AI. Get a publik plan, or use your own key."
        );
        let plan = json!({"claim_state": "claimed", "plan": {"kind": "pro", "label": "Pro"}});
        let s = choose_screen(&plan, None);
        assert_eq!(s.title, CONNECT_TITLE);
        assert_eq!(
            s.body,
            "You are signed in to publik. Your plan: Pro. Connect it to use AI in this app, or use your own key."
        );
        let credit = json!({"plan": {"kind": "none"}, "balance_micros": 2_500_000});
        assert!(choose_screen(&credit, None)
            .body
            .contains("Your balance: $2.50."));
    }

    #[test]
    fn only_the_gate_commands_are_routed_here() {
        assert!(handles("publik_gate_screen"));
        assert!(handles("publik_gate_press"));
        assert!(!handles("fork_get_settings"));
    }
}
