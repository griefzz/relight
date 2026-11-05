//! The window: what it's told when it starts, bringing it back, and quitting.
//!
//! Closed with "keep running" on, it goes altogether rather than just hiding: a loaded page
//! takes far more memory than anything else in relight, which matters when the point of
//! being in the tray is a game running. So what's in the page is kept here meanwhile, and
//! whatever happens while there's no page waits here for the next one.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewWindow, WebviewWindowBuilder};

use crate::recording;

const LABEL: &str = "main";

/// The clip or project named on the command line (`relight clip.mp4`), if any.
pub struct InitialFile(Mutex<Option<String>>);

impl InitialFile {
    pub fn from_command_line() -> Self {
        let path = std::env::args_os()
            .nth(1)
            .and_then(|arg| std::path::absolute(arg).ok())
            .and_then(|path| path.into_os_string().into_string().ok());
        Self(Mutex::new(path))
    }
}

/// The command line's file, given out once, so the window reloading doesn't open it again.
#[tauri::command]
pub fn initial_file(state: State<InitialFile>) -> Option<String> {
    state.0.lock().unwrap().take()
}

/// Something that went wrong starting up (the hotkey being taken), told to the window once.
pub struct Notice(Mutex<Option<String>>);

impl Notice {
    pub fn new(notice: Option<String>) -> Self {
        Self(Mutex::new(notice))
    }
}

#[tauri::command]
pub fn take_notice(state: State<Notice>) -> Option<String> {
    state.0.lock().unwrap().take()
}

/// Whether this copy updates itself. Only the release workflow's builds do: another copy
/// (`npm run install-cli`, a development build) would install a second relight beside itself.
#[tauri::command]
pub fn updates_on() -> bool {
    option_env!("RELIGHT_UPDATES").is_some()
}

/// What's kept for the window while it's gone.
#[derive(Default)]
pub struct Keeper {
    /// The editor as the page last had it, for the next page to take up.
    editor: Mutex<Option<String>>,
    /// Whether that has changes that haven't been saved.
    dirty: AtomicBool,
    place: Mutex<Option<Place>>,
    /// The window's gone on purpose, so relight runs on without one.
    in_tray: AtomicBool,
    /// A new window is being made.
    opening: AtomicBool,
    post: Mutex<Post>,
}

impl Keeper {
    pub fn in_tray(&self) -> bool {
        self.in_tray.load(Ordering::SeqCst)
    }
}

/// Something for the window to know, that it mustn't miss by not being there.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Letter {
    event: &'static str,
    payload: serde_json::Value,
}

/// Letters waiting for a page to be listening.
#[derive(Default)]
struct Post {
    listening: bool,
    waiting: Vec<Letter>,
}

impl Post {
    /// The letter back if someone's listening to hear it; otherwise it's kept for later.
    fn send(&mut self, letter: Letter) -> Option<Letter> {
        if self.listening {
            return Some(letter);
        }
        self.waiting.push(letter);
        None
    }

    /// A page has begun listening: everything that was waiting for one.
    fn collect(&mut self) -> Vec<Letter> {
        self.listening = true;
        std::mem::take(&mut self.waiting)
    }
}

/// Tells the window about something, now if it's there and whenever it's next there if not.
pub fn deliver<T: Serialize + Clone>(app: &AppHandle, event: &'static str, payload: T) {
    let Ok(value) = serde_json::to_value(&payload) else { return };
    let sent = app.state::<Keeper>().post.lock().unwrap().send(Letter { event, payload: value });
    if sent.is_some() {
        let _ = app.emit(event, payload);
    }
}

/// Where the window was, to put the next one back there.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Place {
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    maximized: bool,
}

impl Place {
    /// None if it isn't somewhere it would make sense to come back to.
    fn of(window: &WebviewWindow) -> Option<Self> {
        if window.is_minimized().unwrap_or(true) || window.is_fullscreen().unwrap_or(true) {
            return None;
        }
        Some(Self {
            position: window.outer_position().ok()?,
            size: window.inner_size().ok()?,
            maximized: window.is_maximized().ok()?,
        })
    }

    /// Whether the middle of the window would be on one of these screens, each a position
    /// and size: one may have been unplugged since.
    fn is_on(&self, screens: &[(PhysicalPosition<i32>, PhysicalSize<u32>)]) -> bool {
        let x = i64::from(self.position.x) + i64::from(self.size.width) / 2;
        let y = i64::from(self.position.y) + i64::from(self.size.height) / 2;
        screens.iter().any(|(at, size)| {
            let (left, top) = (i64::from(at.x), i64::from(at.y));
            (left..left + i64::from(size.width)).contains(&x) && (top..top + i64::from(size.height)).contains(&y)
        })
    }

    fn apply(&self, window: &WebviewWindow) {
        let screens = window.available_monitors().unwrap_or_default();
        let screens: Vec<_> = screens.iter().map(|screen| (*screen.position(), *screen.size())).collect();
        if !self.is_on(&screens) {
            return;
        }
        let _ = window.set_position(self.position);
        let _ = window.set_size(self.size);
        if self.maximized {
            let _ = window.maximize();
        }
    }
}

/// Unloads the window to the tray, keeping `editor` for the next one.
#[tauri::command]
pub async fn unload_to_tray(app: AppHandle, editor: String, dirty: bool) -> Result<(), String> {
    let window = app.get_webview_window(LABEL).ok_or("There's no window to unload.")?;
    let keeper = app.state::<Keeper>();
    if let Some(place) = Place::of(&window) {
        *keeper.place.lock().unwrap() = Some(place);
    }
    *keeper.editor.lock().unwrap() = Some(editor);
    keeper.dirty.store(dirty, Ordering::SeqCst);
    keeper.in_tray.store(true, Ordering::SeqCst);
    keeper.post.lock().unwrap().listening = false;
    let destroyed = window.destroy();
    if destroyed.is_err() {
        // It's still here, and still has it all.
        *keeper.editor.lock().unwrap() = None;
        keeper.in_tray.store(false, Ordering::SeqCst);
        keeper.post.lock().unwrap().listening = true;
    }
    destroyed.map_err(|e| e.to_string())
}

/// The editor as the last window left it, given out once.
#[tauri::command]
pub fn take_editor(keeper: State<Keeper>) -> Option<String> {
    keeper.editor.lock().unwrap().take()
}

/// The page is listening: from here on it's told things as they happen, and this is
/// everything that happened while it wasn't.
#[tauri::command]
pub fn window_ready(keeper: State<Keeper>) -> Vec<Letter> {
    keeper.post.lock().unwrap().collect()
}

/// Brings the window back from the tray, the taskbar or behind other windows.
pub fn show_window(app: &AppHandle) {
    match app.get_webview_window(LABEL) {
        Some(window) => reveal(&window),
        None => reopen(app),
    }
}

fn reveal(window: &WebviewWindow) {
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

/// Makes the window again, off this thread: making one on the thread that runs the window
/// events, as a tray click or hotkey does, can deadlock on Windows (wry issue 583).
fn reopen(app: &AppHandle) {
    if app.state::<Keeper>().opening.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    thread::spawn(move || {
        let keeper = app.state::<Keeper>();
        match build(&app) {
            Ok(window) => {
                if let Some(place) = *keeper.place.lock().unwrap() {
                    place.apply(&window);
                }
                keeper.in_tray.store(false, Ordering::SeqCst);
                reveal(&window);
            }
            // It stays in the tray, where another click can try again.
            Err(e) => eprintln!("Couldn't open the window: {e}"),
        }
        keeper.opening.store(false, Ordering::SeqCst);
    });
}

/// The window as configured, hidden until it's been put in its place. The last one's name may
/// not have been let go of yet, if this comes straight after it went.
fn build(app: &AppHandle) -> Result<WebviewWindow, String> {
    let config = app.config().app.windows.first().ok_or("There's no window configured.")?;
    let mut tries = 20;
    loop {
        let built = WebviewWindowBuilder::from_config(app, config)
            .and_then(|builder| builder.visible(false).build())
            .map_err(|e| e.to_string());
        tries -= 1;
        if built.is_ok() || tries == 0 {
            return built;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Whether quitting has nothing to ask the user about, so no window's needed to ask it.
fn can_quit_unasked(in_tray: bool, opening: bool, dirty: bool) -> bool {
    in_tray && !opening && !dirty
}

/// Quit, picked from the tray: the window decides, as it may have unsaved changes to ask
/// about. With no window there, and nothing unsaved, there's no need to make one to ask.
pub fn ask_to_quit(app: &AppHandle) {
    let keeper = app.state::<Keeper>();
    let (in_tray, opening, dirty) = (keeper.in_tray(), keeper.opening.load(Ordering::SeqCst), keeper.dirty.load(Ordering::SeqCst));
    if can_quit_unasked(in_tray, opening, dirty) {
        let app = app.clone();
        thread::spawn(move || {
            recording::stop_and_wait(&app);
            app.exit(0);
        });
        return;
    }
    show_window(app);
    deliver(app, "quit-requested", ());
}

/// Quits, once any recording has been saved. Off the main thread, so the window keeps
/// drawing while it waits.
#[tauri::command]
pub async fn quit(app: AppHandle) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        recording::stop_and_wait(&app);
        app.exit(0);
    })
    .await;
}

/// `relight clip.mp4` typed with relight already running: the running one shows itself and
/// opens it, rather than a second copy starting and fighting it for the hotkey.
pub fn another_instance(app: &AppHandle, args: Vec<String>, cwd: String) {
    show_window(app);
    if let Some(file) = args.get(1) {
        let path = Path::new(&cwd).join(file);
        deliver(app, "open-file", path.to_string_lossy().into_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter(event: &'static str) -> Letter {
        Letter { event, payload: serde_json::Value::Null }
    }

    #[test]
    fn with_no_page_listening_letters_wait_in_order() {
        let mut post = Post::default();
        assert_eq!(post.send(letter("a")), None);
        assert_eq!(post.send(letter("b")), None);
        assert_eq!(post.collect(), [letter("a"), letter("b")]);
        assert!(post.collect().is_empty(), "they're only handed over once");
    }

    #[test]
    fn with_a_page_listening_letters_go_straight_to_it() {
        let mut post = Post::default();
        post.collect();
        assert_eq!(post.send(letter("a")), Some(letter("a")));
        assert!(post.waiting.is_empty());
    }

    #[test]
    fn letters_wait_again_once_the_page_has_gone() {
        let mut post = Post::default();
        post.collect();
        post.listening = false;
        assert_eq!(post.send(letter("a")), None);
        assert_eq!(post.collect(), [letter("a")]);
    }

    #[test]
    fn a_window_comes_back_only_to_a_screen_thats_still_there() {
        let screen = |x, y, w, h| (PhysicalPosition::new(x, y), PhysicalSize::new(w, h));
        let place = |x, y| Place { position: PhysicalPosition::new(x, y), size: PhysicalSize::new(1000, 600), maximized: false };
        let screens = [screen(0, 0, 1920, 1080), screen(1920, 0, 2560, 1440)];
        assert!(place(100, 100).is_on(&screens));
        assert!(place(2500, 100).is_on(&screens), "on the second screen");
        // A maximized window's corner hangs just off the screen, but its middle's on it.
        assert!(place(-8, -8).is_on(&screens));
        assert!(!place(5000, 100).is_on(&screens), "the screen it was on has gone");
        assert!(!place(100, 2000).is_on(&screens));
        assert!(!place(100, 100).is_on(&[]));
    }

    #[test]
    fn quitting_only_needs_a_window_when_theres_something_to_ask() {
        assert!(can_quit_unasked(true, false, false));
        assert!(!can_quit_unasked(true, false, true), "unsaved changes want asking about");
        assert!(!can_quit_unasked(false, false, false), "the window's there to decide");
        assert!(!can_quit_unasked(true, true, false), "a window's on its way");
    }
}
