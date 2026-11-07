//! The phone driven by another device, through the host (LP-167).
//!
//! Android lets an app inject input only as an accessibility service's
//! gestures and input connection, and only the app can be one. So what a
//! controller sends is translated here into [`DriveAction`]s the host carries
//! out: a touch screen has no pointer and no buttons, so the left button is a
//! finger at the pointer, the right is Back and the middle is Home; scrolling
//! is a swipe the other way; a character is committed as text.

use std::sync::{Arc, Mutex};

use lp_core::Error;
use lp_core::input::keyid::{self, KeyId, mask};
use lp_core::input::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, InputEvent, InputParams};

use ligatured::input::Inject;

use crate::SharedHost;

/// How far a wheel step scrolls, in pixels.
const WHEEL_STEP_PX: f64 = 160.0;
/// The shortest swipe worth making: less is under Android's touch slop and
/// would be a tap. Scrolling adds up until it is reached.
const MIN_SWIPE_PX: f64 = 48.0;

/// A key the host presses with the service's input connection.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum DriveKey {
    /// Backspace.
    Backspace,
    /// Forward delete.
    Delete,
    /// Return or Enter.
    Enter,
    /// Tab.
    Tab,
    /// Left.
    Left,
    /// Up.
    Up,
    /// Right.
    Right,
    /// Down.
    Down,
    /// Home, in the text being edited.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// A character pressed as a shortcut, such as the C of Control-C.
    Char {
        /// The character.
        text: String,
    },
}

/// One thing a drive asks of the phone.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum DriveAction {
    /// A drive began: show the pointer at this position, in pixels.
    Begin {
        /// Across.
        x: f64,
        /// Down.
        y: f64,
    },
    /// The pointer moved with no finger down.
    Pointer {
        /// Across.
        x: f64,
        /// Down.
        y: f64,
    },
    /// A finger goes down.
    TouchDown {
        /// Across.
        x: f64,
        /// Down.
        y: f64,
    },
    /// The finger moves.
    TouchMove {
        /// Across.
        x: f64,
        /// Down.
        y: f64,
    },
    /// The finger lifts.
    TouchUp {
        /// Across.
        x: f64,
        /// Down.
        y: f64,
    },
    /// A whole swipe from a point by a distance, in pixels.
    Swipe {
        /// Across, at the start.
        x: f64,
        /// Down, at the start.
        y: f64,
        /// How far across.
        dx: f64,
        /// How far down.
        dy: f64,
    },
    /// Characters to commit to the focused field.
    Text {
        /// The characters.
        text: String,
    },
    /// A key to press and release in the focused field.
    Key {
        /// The key.
        key: DriveKey,
        /// Shift is held.
        shift: bool,
        /// A command modifier is held: Control, or a Mac's Command.
        control: bool,
        /// Alt is held.
        alt: bool,
    },
    /// The system's Back.
    Back,
    /// The system's Home.
    Home,
    /// The drive ended: hide the pointer.
    End,
}

/// This screen's size in pixels as it is turned now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct DriveScreen {
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

fn named_key(id: KeyId) -> Option<DriveKey> {
    Some(match id {
        keyid::BACKSPACE => DriveKey::Backspace,
        keyid::DELETE => DriveKey::Delete,
        keyid::RETURN | keyid::KP_ENTER => DriveKey::Enter,
        keyid::TAB => DriveKey::Tab,
        keyid::LEFT => DriveKey::Left,
        keyid::UP => DriveKey::Up,
        keyid::RIGHT => DriveKey::Right,
        keyid::DOWN => DriveKey::Down,
        keyid::HOME => DriveKey::Home,
        keyid::END => DriveKey::End,
        keyid::PAGE_UP => DriveKey::PageUp,
        keyid::PAGE_DOWN => DriveKey::PageDown,
        _ => return None,
    })
}

/// A drive in progress: the pointer, whether a finger is down, and the
/// scrolling not yet worth a swipe.
#[derive(Debug)]
struct Translator {
    width: f64,
    height: f64,
    x: f64,
    y: f64,
    touching: bool,
    scroll_x: f64,
    scroll_y: f64,
}

impl Translator {
    fn new(screen: DriveScreen, start: (f64, f64)) -> Self {
        let width = f64::from(screen.width.max(1));
        let height = f64::from(screen.height.max(1));
        Self {
            width,
            height,
            x: (start.0 * width).clamp(0.0, width - 1.0),
            y: (start.1 * height).clamp(0.0, height - 1.0),
            touching: false,
            scroll_x: 0.0,
            scroll_y: 0.0,
        }
    }

    fn moved(&self) -> DriveAction {
        let (x, y) = (self.x, self.y);
        if self.touching {
            DriveAction::TouchMove { x, y }
        } else {
            DriveAction::Pointer { x, y }
        }
    }

    fn event(&mut self, event: &InputEvent) -> Vec<DriveAction> {
        match event {
            InputEvent::Motion { dx, dy } => {
                self.x = (self.x + dx).clamp(0.0, self.width - 1.0);
                self.y = (self.y + dy).clamp(0.0, self.height - 1.0);
                vec![self.moved()]
            }
            InputEvent::Position { x, y } => {
                self.x = (x * self.width).clamp(0.0, self.width - 1.0);
                self.y = (y * self.height).clamp(0.0, self.height - 1.0);
                vec![self.moved()]
            }
            InputEvent::Button { code, pressed } => self.button(*code, *pressed),
            InputEvent::Scroll { dx, dy, discrete } => self.scroll(*dx, *dy, *discrete),
            InputEvent::Key {
                code,
                pressed: true,
                id,
                mods,
            } => {
                let id = id.or_else(|| keyid::named_for_evdev(*code));
                id.and_then(|id| Self::key(id, mods.unwrap_or(0)))
                    .into_iter()
                    .collect()
            }
            // A press does the whole key; Android has no held keys to lift.
            InputEvent::Key { .. } => Vec::new(),
            InputEvent::Text { text } => vec![DriveAction::Text { text: text.clone() }],
        }
    }

    fn button(&mut self, code: u16, pressed: bool) -> Vec<DriveAction> {
        let (x, y) = (self.x, self.y);
        match (code, pressed) {
            (BTN_LEFT, true) if !self.touching => {
                self.touching = true;
                vec![DriveAction::TouchDown { x, y }]
            }
            (BTN_LEFT, false) if self.touching => {
                self.touching = false;
                vec![DriveAction::TouchUp { x, y }]
            }
            (BTN_RIGHT, true) => vec![DriveAction::Back],
            (BTN_MIDDLE, true) => vec![DriveAction::Home],
            _ => Vec::new(),
        }
    }

    fn scroll(&mut self, dx: f64, dy: f64, discrete: bool) -> Vec<DriveAction> {
        if self.touching {
            return Vec::new();
        }
        let scale = if discrete { WHEEL_STEP_PX } else { 1.0 };
        self.scroll_x += dx * scale;
        self.scroll_y += dy * scale;
        if self.scroll_x.abs() < MIN_SWIPE_PX && self.scroll_y.abs() < MIN_SWIPE_PX {
            return Vec::new();
        }
        // Scrolling down moves the content up: the finger goes up.
        let swipe = DriveAction::Swipe {
            x: self.x,
            y: self.y,
            dx: -self.scroll_x,
            dy: -self.scroll_y,
        };
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        vec![swipe]
    }

    fn key(id: KeyId, mods: u32) -> Option<DriveAction> {
        if keyid::is_modifier(id) || keyid::is_lock(id) {
            return None;
        }
        let control = mods & (mask::CONTROL | mask::META | mask::SUPER) != 0;
        let alt = mods & mask::ALT != 0;
        let shift = mods & mask::SHIFT != 0;
        if id == keyid::ESCAPE {
            return Some(DriveAction::Back);
        }
        if let Some(key) = named_key(id) {
            return Some(DriveAction::Key {
                key,
                shift,
                control,
                alt,
            });
        }
        let character = keyid::character(id)?;
        if control || alt {
            Some(DriveAction::Key {
                key: DriveKey::Char {
                    text: character.to_string(),
                },
                shift,
                control,
                alt,
            })
        } else {
            Some(DriveAction::Text {
                text: character.to_string(),
            })
        }
    }

    fn end(&mut self) -> Vec<DriveAction> {
        let mut actions = self.button(BTN_LEFT, false);
        actions.push(DriveAction::End);
        actions
    }
}

#[derive(Default)]
struct State {
    accepts: bool,
    text: bool,
    drive: Option<Translator>,
}

/// The phone's input system: the app's accessibility service.
pub struct HostInput {
    host: SharedHost,
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for HostInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostInput")
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl HostInput {
    /// An injector through `host`; it accepts nothing until the host says it
    /// can with [`HostInput::set_drivable`].
    #[must_use]
    pub fn new(host: SharedHost) -> Arc<Self> {
        Arc::new(Self {
            host,
            state: Arc::new(Mutex::new(State::default())),
        })
    }

    /// Whether the user has enabled the accessibility service, and whether
    /// it can type (it needs Android 13).
    pub fn set_drivable(&self, accepts: bool, text: bool) {
        let mut s = lock(&self.state);
        s.accepts = accepts;
        s.text = text;
    }

    fn perform(&self, actions: Vec<DriveAction>) {
        for action in actions {
            self.host.drive(action);
        }
    }
}

impl Inject for HostInput {
    fn params(&self) -> InputParams {
        let s = lock(&self.state);
        InputParams {
            accepts: s.accepts,
            provides: false,
            absolute: s.accepts,
            text: s.accepts && s.text,
        }
    }

    fn screen(&self) -> (u32, u32) {
        let screen = self.host.drive_screen();
        (screen.width, screen.height)
    }

    fn begin(&self, start: (f64, f64)) -> Result<(), Error> {
        let drive = Translator::new(self.host.drive_screen(), start);
        let begin = DriveAction::Begin {
            x: drive.x,
            y: drive.y,
        };
        lock(&self.state).drive = Some(drive);
        self.perform(vec![begin]);
        Ok(())
    }

    fn inject(&self, event: &InputEvent) -> Result<(), Error> {
        let actions = lock(&self.state)
            .drive
            .as_mut()
            .map(|d| d.event(event))
            .unwrap_or_default();
        self.perform(actions);
        Ok(())
    }

    fn end(&self) {
        let actions = lock(&self.state)
            .drive
            .take()
            .map(|mut d| d.end())
            .unwrap_or_default();
        self.perform(actions);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive() -> Translator {
        Translator::new(
            DriveScreen {
                width: 1000,
                height: 2000,
            },
            (0.5, 0.5),
        )
    }

    fn key(id: KeyId, mods: u32) -> InputEvent {
        InputEvent::Key {
            code: 30,
            pressed: true,
            id: Some(id),
            mods: Some(mods),
        }
    }

    #[test]
    fn a_position_moves_the_pointer_without_touching() {
        let mut d = drive();
        assert_eq!(
            d.event(&InputEvent::Position { x: 0.1, y: 0.25 }),
            vec![DriveAction::Pointer { x: 100.0, y: 500.0 }]
        );
    }

    #[test]
    fn motion_stays_on_the_screen() {
        let mut d = drive();
        assert_eq!(
            d.event(&InputEvent::Motion {
                dx: -5000.0,
                dy: 5000.0
            }),
            vec![DriveAction::Pointer { x: 0.0, y: 1999.0 }]
        );
    }

    #[test]
    fn the_left_button_is_a_finger_that_drags() {
        let mut d = drive();
        let press = InputEvent::Button {
            code: BTN_LEFT,
            pressed: true,
        };
        assert_eq!(
            d.event(&press),
            vec![DriveAction::TouchDown {
                x: 500.0,
                y: 1000.0
            }]
        );
        assert_eq!(d.event(&press), Vec::new(), "a second press is no finger");
        assert_eq!(
            d.event(&InputEvent::Motion { dx: 10.0, dy: 0.0 }),
            vec![DriveAction::TouchMove {
                x: 510.0,
                y: 1000.0
            }]
        );
        assert_eq!(
            d.event(&InputEvent::Button {
                code: BTN_LEFT,
                pressed: false
            }),
            vec![DriveAction::TouchUp {
                x: 510.0,
                y: 1000.0
            }]
        );
    }

    #[test]
    fn the_other_buttons_are_back_and_home_once_each() {
        let mut d = drive();
        let click = |code, pressed| InputEvent::Button { code, pressed };
        assert_eq!(d.event(&click(BTN_RIGHT, true)), vec![DriveAction::Back]);
        assert_eq!(d.event(&click(BTN_RIGHT, false)), Vec::new());
        assert_eq!(d.event(&click(BTN_MIDDLE, true)), vec![DriveAction::Home]);
        assert_eq!(d.event(&click(BTN_MIDDLE, false)), Vec::new());
    }

    #[test]
    fn scrolling_down_swipes_up_once_it_is_worth_a_swipe() {
        let mut d = drive();
        let pixels = |dy| InputEvent::Scroll {
            dx: 0.0,
            dy,
            discrete: false,
        };
        assert_eq!(d.event(&pixels(30.0)), Vec::new());
        assert_eq!(
            d.event(&pixels(30.0)),
            vec![DriveAction::Swipe {
                x: 500.0,
                y: 1000.0,
                dx: 0.0,
                dy: -60.0
            }]
        );
        assert_eq!(d.event(&pixels(30.0)), Vec::new(), "the sum starts again");
    }

    #[test]
    fn a_wheel_step_is_a_swipe_of_its_own() {
        let mut d = drive();
        assert_eq!(
            d.event(&InputEvent::Scroll {
                dx: 0.0,
                dy: -1.0,
                discrete: true
            }),
            vec![DriveAction::Swipe {
                x: 500.0,
                y: 1000.0,
                dx: 0.0,
                dy: WHEEL_STEP_PX
            }]
        );
    }

    #[test]
    fn scrolling_with_a_finger_down_does_nothing() {
        let mut d = drive();
        d.event(&InputEvent::Button {
            code: BTN_LEFT,
            pressed: true,
        });
        assert_eq!(
            d.event(&InputEvent::Scroll {
                dx: 0.0,
                dy: 5.0,
                discrete: true
            }),
            Vec::new()
        );
    }

    #[test]
    fn a_character_is_text_whatever_shift_made_it() {
        let mut d = drive();
        assert_eq!(
            d.event(&key(u32::from('A'), mask::SHIFT)),
            vec![DriveAction::Text {
                text: "A".to_owned()
            }]
        );
    }

    #[test]
    fn a_shortcut_is_a_key_and_commands_count_as_control() {
        let mut d = drive();
        let expected = vec![DriveAction::Key {
            key: DriveKey::Char {
                text: "c".to_owned(),
            },
            shift: false,
            control: true,
            alt: false,
        }];
        assert_eq!(d.event(&key(u32::from('c'), mask::CONTROL)), expected);
        assert_eq!(d.event(&key(u32::from('c'), mask::SUPER)), expected);
    }

    #[test]
    fn a_named_key_is_a_key_and_escape_is_back() {
        let mut d = drive();
        assert_eq!(
            d.event(&key(keyid::BACKSPACE, 0)),
            vec![DriveAction::Key {
                key: DriveKey::Backspace,
                shift: false,
                control: false,
                alt: false
            }]
        );
        assert_eq!(d.event(&key(keyid::ESCAPE, 0)), vec![DriveAction::Back]);
    }

    #[test]
    fn a_modifier_alone_and_a_release_do_nothing() {
        let mut d = drive();
        assert_eq!(d.event(&key(keyid::SHIFT_L, 0)), Vec::new());
        assert_eq!(d.event(&key(keyid::CAPS_LOCK, 0)), Vec::new());
        assert_eq!(d.event(&InputEvent::key(30, false)), Vec::new());
    }

    #[test]
    fn a_key_with_no_meaning_is_placed_by_its_code_or_dropped() {
        let mut d = drive();
        // 14 is Backspace by position, 30 is a letter this device cannot place.
        let backspace = d.event(&InputEvent::key(14, true));
        assert!(
            matches!(
                backspace.as_slice(),
                [DriveAction::Key {
                    key: DriveKey::Backspace,
                    ..
                }]
            ),
            "{backspace:?}"
        );
        assert_eq!(d.event(&InputEvent::key(30, true)), Vec::new());
    }

    #[test]
    fn ending_lifts_a_finger_still_down_then_hides_the_pointer() {
        let mut d = drive();
        d.event(&InputEvent::Button {
            code: BTN_LEFT,
            pressed: true,
        });
        assert_eq!(
            d.end(),
            vec![
                DriveAction::TouchUp {
                    x: 500.0,
                    y: 1000.0
                },
                DriveAction::End
            ]
        );
        assert_eq!(drive().end(), vec![DriveAction::End]);
    }
}
