//! The document on paper, the Mac's way: File ▸ Print… through the print
//! panel, and File ▸ Export as PDF… through a save sheet. Both are the PDF
//! `pdf.rs` has twig-export set — Print's on the paper and margins Page
//! Setup chose, Export's on the paper Settings names.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{NSModalResponseOK, NSPrintInfo, NSPrintOperation, NSSavePanel};
use objc2_foundation::{NSArray, NSData, NSString};
use objc2_pdf_kit::{PDFDocument, PDFPrintScalingMode};

use crate::page::Page;
use crate::pdf::Paper;

/// The print operation `NSDocument`'s Print runs: the document's PDF, on
/// the printer's paper.
pub fn operation(page: &Page, info: &NSPrintInfo) -> Option<Retained<NSPrintOperation>> {
    let size = info.paperSize();
    let paper = Paper {
        width: size.width as f32,
        height: size.height as f32,
        margin: info.topMargin().max(36.0) as f32,
    };
    let bytes = match crate::pdf::of_page(page, paper) {
        Ok(bytes) => bytes,
        Err(e) => {
            page.say(&format!("Print failed: {e}"));
            return None;
        }
    };
    let data = NSData::with_bytes(&bytes);
    let pdf = unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }?;
    unsafe {
        pdf.printOperationForPrintInfo_scalingMode_autoRotate(
            Some(info),
            PDFPrintScalingMode::PageScaleNone,
            false,
            super::mtm(),
        )
    }
}

/// File ▸ Export as PDF…: a save sheet on the window, then the PDF on the
/// paper Settings names.
pub fn export(page: &Page) {
    let id = page.window.id();
    let Some(window) = super::ns_window(&*page.window) else {
        return;
    };
    let name = page
        .shell()
        .map(|s| s.document.title())
        .unwrap_or_else(|| "Untitled".into());
    let stem = name
        .rsplit_once('.')
        .map_or(name.as_str(), |(s, _)| s)
        .to_string();
    super::later(move || {
        let panel = NSSavePanel::savePanel(super::mtm());
        // `allowedContentTypes` would want UniformTypeIdentifiers for one
        // type; the extension says the same.
        #[allow(deprecated)]
        panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&[NSString::from_str(
            "pdf",
        )])));
        panel.setCanCreateDirectories(true);
        panel.setNameFieldStringValue(&NSString::from_str(&format!("{stem}.pdf")));
        let chosen = panel.clone();
        let done = block2::RcBlock::new(move |response| {
            if response != NSModalResponseOK {
                return;
            }
            let Some(path) = chosen.URL().and_then(|u| u.path()) else {
                return;
            };
            let path = std::path::PathBuf::from(path.to_string());
            crate::app::on_turn(move || {
                let Some(page) = crate::app::page(id) else {
                    return;
                };
                let paper = Paper::of(&crate::app::settings());
                match crate::pdf::of_page(&page, paper) {
                    Ok(bytes) => {
                        if let Err(e) = std::fs::write(&path, bytes) {
                            page.say(&format!("Export failed: {e}"));
                        }
                    }
                    Err(e) => page.say(&format!("Export failed: {e}")),
                }
            });
        });
        panel.beginSheetModalForWindow_completionHandler(&window, &done);
    });
}
