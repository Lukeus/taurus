//! A picture of a webview, as PNG bytes.
//!
//! wry wraps no snapshot API on any desktop engine, so each one is called
//! directly: `WKWebView` here, and in time `ICoreWebView2::CapturePreview` on
//! Windows and `webkit2gtk::WebView::snapshot` on Linux. That means FFI, which
//! the rest of the workspace forbids. It lives here so the exception is one
//! crate with a safe surface — `snapshot` takes a `Webview`, never a pointer —
//! rather than a lint lifted in the app.

use std::time::Duration;

use tauri::{Runtime, Webview};

/// How long the engine gets to hand the picture back.
const WAIT: Duration = Duration::from_secs(5);

/// A rectangle of the webview, in its own points: what a CSS pixel is at 100%
/// zoom, measured from the webview's top left.
#[derive(Clone, Copy, Debug)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Draw it this many points wide, scaling the height to match, instead
    /// of at its own width. For a picture headed somewhere that pays by the
    /// pixel. `None` keeps its own.
    pub fit_width: Option<f64>,
}

/// The webview as it's drawn, or the `area` of it, at its own scale, as PNG.
///
/// Whatever is on screen there is in the picture, including a cross-origin
/// iframe, which no script in the page could photograph.
pub async fn snapshot<R: Runtime>(
    webview: &Webview<R>,
    area: Option<Area>,
) -> Result<Vec<u8>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<Vec<u8>, String>>();
    platform::snapshot(webview, area, tx)?;
    tokio::time::timeout(WAIT, rx)
        .await
        .map_err(|_| {
            format!(
                "the picture didn't arrive within {} seconds",
                WAIT.as_secs()
            )
        })?
        .map_err(|_| "the picture was dropped before it arrived".to_string())?
}

type Reply = tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>;

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod platform {
    use std::sync::{Arc, Mutex};

    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_foundation::{NSDictionary, NSError, NSNumber};
    use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};
    use tauri::{Runtime, Webview};

    use super::{Area, Reply};

    pub fn snapshot<R: Runtime>(
        webview: &Webview<R>,
        area: Option<Area>,
        reply: Reply,
    ) -> Result<(), String> {
        // The completion handler is an `Fn`, and WebKit promises to call it
        // once; the `Option` makes "once" true whatever it does.
        let reply = Arc::new(Mutex::new(Some(reply)));
        let send = move |result: Result<Vec<u8>, String>| {
            if let Some(tx) = reply.lock().ok().and_then(|mut slot| slot.take()) {
                let _ = tx.send(result);
            }
        };

        // `with_webview` runs this on the main thread, which is where WebKit
        // must be called, and where it calls the handler back.
        webview
            .with_webview(move |platform| {
                // SAFETY: on macOS `inner()` is the `WKWebView*` wry created
                // and still owns. It outlives this closure, which is the only
                // place the reference is used.
                let view: &WKWebView = unsafe { &*platform.inner().cast::<WKWebView>() };
                let done = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
                    // SAFETY: WebKit passes either a valid image or a valid
                    // error (or, defensively, neither), each live for the
                    // duration of this call. `as_ref` turns null into `None`.
                    let (image, error) = unsafe { (image.as_ref(), error.as_ref()) };
                    send(match (image, error) {
                        (Some(image), _) => png(image),
                        (None, Some(error)) => Err(format!(
                            "WebKit couldn't take the picture: {}",
                            error.localizedDescription()
                        )),
                        (None, None) => {
                            Err("WebKit returned neither a picture nor an error".into())
                        }
                    });
                });
                // No configuration means the visible bounds. With one, `rect` is
                // in the view's coordinates, which is what `Area` is.
                let main = MainThreadMarker::new();
                let config = area.zip(main).map(|(area, mtm)| {
                    // SAFETY: `mtm` proves the main thread; a fresh
                    // configuration is ours alone.
                    let config = unsafe { WKSnapshotConfiguration::new(mtm) };
                    // SAFETY: as above. A rect outside the view is clipped
                    // by WebKit, not undefined.
                    unsafe {
                        config.setRect(CGRect::new(
                            CGPoint::new(area.x, area.y),
                            CGSize::new(area.width, area.height),
                        ));
                    }
                    if let Some(width) = area.fit_width {
                        // SAFETY: as above; WebKit copies the number.
                        unsafe {
                            config.setSnapshotWidth(Some(&NSNumber::numberWithDouble(width)))
                        };
                    }
                    config
                });
                // SAFETY: called on the main thread (see above), with a
                // configuration that lives until the call returns, after
                // which WebKit holds its own reference.
                unsafe {
                    view.takeSnapshotWithConfiguration_completionHandler(config.as_deref(), &done)
                };
            })
            .map_err(|e| e.to_string())
    }

    fn png(image: &NSImage) -> Result<Vec<u8>, String> {
        let tiff = image
            .TIFFRepresentation()
            .ok_or("the picture had no bitmap")?;
        let rep = NSBitmapImageRep::imageRepWithData(&tiff).ok_or("the bitmap couldn't be read")?;
        // SAFETY: an empty dictionary is a valid set of properties for any
        // file type.
        let data = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }
        .ok_or("the bitmap couldn't be written as PNG")?;
        Ok(data.to_vec())
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use tauri::{Runtime, Webview};

    use super::{Area, Reply};

    pub fn snapshot<R: Runtime>(
        _webview: &Webview<R>,
        _area: Option<Area>,
        _reply: Reply,
    ) -> Result<(), String> {
        Err("taking a picture of a webview is only written for macOS so far".into())
    }
}
