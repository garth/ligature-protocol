//! `UNUserNotificationCenter` through `objc2` (LP-94).
//!
//! The centre serves only a process inside an application bundle: asked for
//! by a bare executable it raises an Objective-C exception, which would abort
//! the daemon. [`bundle_identifier`] says whether there is one, and [`start`]
//! refuses without it, so nothing here touches the centre outside a bundle.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_foundation::{
    NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSSet, NSString, NSURL,
};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAttachment,
    UNNotificationCategory, UNNotificationCategoryOptions, UNNotificationDefaultActionIdentifier,
    UNNotificationDismissActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

/// The category every posting carries. It asks for the custom dismiss action,
/// without which the centre does not tell the delegate that the user closed a
/// notification.
pub const CATEGORY: &str = "org.ligatureprotocol.copy";

/// What the user did with a posted notification, by its identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// The user closed it.
    Dismissed(String),
    /// The user clicked it, which also removes it.
    Opened(String),
}

/// Told of every [`Response`]. Called on whatever thread the centre uses.
pub type Responder = Arc<dyn Fn(Response) + Send + Sync>;

/// Told once whether the user allows notifications: `true` when allowed, and
/// the centre's error, if any. Called on whatever thread the centre uses.
pub type Authorised = Arc<dyn Fn(bool, Option<String>) + Send + Sync>;

/// Told of a posting the centre refused, with its error.
pub type Refused = Arc<dyn Fn(String) + Send + Sync>;

/// One notification to post. A posting with the identifier of one already
/// shown replaces it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Posting {
    /// Unique among the process's notifications; [`remove`] takes it.
    pub identifier: String,
    /// Notifications with the same thread are grouped.
    pub thread: String,
    /// The first line.
    pub title: String,
    /// The second line, if any.
    pub subtitle: Option<String>,
    /// The text, if any.
    pub body: Option<String>,
    /// An image file to attach. The centre moves the file into its own store
    /// when it accepts the posting, so it should be a copy made for this one.
    pub image: Option<PathBuf>,
}

/// The identifier of the bundle this process runs from, if it runs from one.
#[must_use]
pub fn bundle_identifier() -> Option<String> {
    NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|s| s.to_string())
}

struct DelegateIvars {
    responder: Responder,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class does
    // not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "LigatureNotificationDelegate"]
    #[ivars = DelegateIvars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        // An agent with no windows may count as frontmost; without this the
        // centre would then file the notification without showing it.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler
                .call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let identifier = response.notification().request().identifier().to_string();
            let action = response.actionIdentifier();
            // SAFETY: both are constant strings the framework exports.
            let (dismiss, default) = unsafe {
                (
                    UNNotificationDismissActionIdentifier,
                    UNNotificationDefaultActionIdentifier,
                )
            };
            if *action == *dismiss {
                (self.ivars().responder)(Response::Dismissed(identifier));
            } else if *action == *default {
                (self.ivars().responder)(Response::Opened(identifier));
            }
            handler.call(());
        }
    }
);

impl Delegate {
    fn new(responder: Responder) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { responder });
        // SAFETY: NSObject's init takes no arguments.
        unsafe { msg_send![super(this), init] }
    }
}

/// The delegate, which the centre holds only weakly.
struct Keep(#[allow(dead_code)] Retained<Delegate>);

// SAFETY: the delegate is only ever called by the centre and holds a
// `Send + Sync` function; the retained pointer is moved between threads only
// to be replaced.
unsafe impl Send for Keep {}

static DELEGATE: Mutex<Option<Keep>> = Mutex::new(None);

/// Starts using the centre: sets the delegate that reports what the user does
/// with a notification, registers [`CATEGORY`], and asks the user for
/// permission, which shows the prompt the first time under the bundle's name.
///
/// # Errors
///
/// Returns why when the process does not run from a bundle; the centre is
/// then never touched.
pub fn start(responder: Responder, authorised: Authorised) -> Result<(), String> {
    if bundle_identifier().is_none() {
        return Err("not running from an application bundle".to_owned());
    }
    let centre = UNUserNotificationCenter::currentNotificationCenter();
    let delegate = Delegate::new(responder);
    centre.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    *DELEGATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Keep(delegate));

    let category = UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(CATEGORY),
        &NSArray::new(),
        &NSArray::new(),
        UNNotificationCategoryOptions::CustomDismissAction,
    );
    centre.setNotificationCategories(&NSSet::from_retained_slice(&[category]));

    let block = RcBlock::new(move |granted: Bool, error: *mut NSError| {
        // SAFETY: the centre passes null or a valid error for the block's
        // duration.
        let error = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
        authorised(granted.as_bool(), error);
    });
    centre.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert, &block);
    Ok(())
}

/// Posts `posting`, replacing a shown one with the same identifier. Call only
/// after [`start`] succeeded. An attachment the centre cannot read is left
/// out and reported to `refused`, as is a posting it refuses.
pub fn post(posting: &Posting, refused: Refused) {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&posting.title));
    if let Some(subtitle) = &posting.subtitle {
        content.setSubtitle(&NSString::from_str(subtitle));
    }
    if let Some(body) = &posting.body {
        content.setBody(&NSString::from_str(body));
    }
    content.setThreadIdentifier(&NSString::from_str(&posting.thread));
    content.setCategoryIdentifier(&NSString::from_str(CATEGORY));
    if let Some(path) = &posting.image {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        // SAFETY: no options are passed, so there is no dictionary whose
        // value types could be wrong.
        match unsafe {
            UNNotificationAttachment::attachmentWithIdentifier_URL_options_error(
                &NSString::from_str("icon"),
                &url,
                None,
            )
        } {
            Ok(attachment) => content.setAttachments(&NSArray::from_retained_slice(&[attachment])),
            Err(e) => refused(format!("icon not attached: {}", e.localizedDescription())),
        }
    }
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&posting.identifier),
        &content,
        None,
    );
    let block = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: as in `start`.
        if let Some(e) = unsafe { error.as_ref() } {
            refused(e.localizedDescription().to_string());
        }
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, Some(&block));
}

/// Removes the shown notifications with these identifiers; one not shown is
/// ignored. Call only after [`start`] succeeded.
pub fn remove(identifiers: &[&str]) {
    let ids: Vec<Retained<NSString>> = identifiers.iter().map(|s| NSString::from_str(s)).collect();
    UNUserNotificationCenter::currentNotificationCenter()
        .removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&ids));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_binary_has_no_bundle_and_start_refuses_without_touching_the_centre() {
        assert_eq!(bundle_identifier(), None);
        let err = start(Arc::new(|_| {}), Arc::new(|_, _| {})).unwrap_err();
        assert!(err.contains("bundle"), "{err}");
    }
}
