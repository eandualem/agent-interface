use core::slice;
use std::ffi::{CStr, c_void};
use std::io::Read;
use std::path::PathBuf;

use cocoa::{
    appkit::{
        NSFilenamesPboardType, NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeString,
        NSPasteboardTypeTIFF,
    },
    base::{id, nil},
    foundation::{NSArray, NSData, NSFastEnumeration, NSString},
};
use objc::{class, msg_send, rc::StrongPtr, runtime::Object, sel, sel_impl};
use image::ImageDecoder as _;
use smallvec::SmallVec;
use strum::IntoEnumIterator as _;

use crate::ns_string;
use gpui::{
    ClipboardEntry, ClipboardItem, ClipboardString, ExternalPaths, Image, ImageFormat, hash,
};

pub struct Pasteboard {
    inner: StrongPtr,
    text_hash_type: StrongPtr,
    metadata_type: StrongPtr,
}

impl Pasteboard {
    pub fn general() -> Self {
        unsafe { Self::new(NSPasteboard::generalPasteboard(nil)) }
    }

    pub fn find() -> Self {
        unsafe { Self::new(NSPasteboard::pasteboardWithName(nil, NSPasteboardNameFind)) }
    }

    #[cfg(test)]
    pub fn unique() -> Self {
        unsafe { Self::new(NSPasteboard::pasteboardWithUniqueName(nil)) }
    }

    /// The caller supplies a live AppKit pasteboard object on the main thread.
    pub(crate) unsafe fn new(inner: id) -> Self {
        // These constructors return autoreleased objects, but a Pasteboard can
        // outlive the autorelease pool in which it was created.
        Self {
            inner: unsafe { StrongPtr::retain(inner) },
            text_hash_type: unsafe { StrongPtr::retain(ns_string("zed-text-hash")) },
            metadata_type: unsafe { StrongPtr::retain(ns_string("zed-metadata")) },
        }
    }

    pub fn read(&self) -> Option<ClipboardItem> {
        unsafe {
            if let Some(paths) = self.read_paths() {
                let mut entries = vec![ClipboardEntry::ExternalPaths(paths)];
                if let Some(string_item) = self.read_string_from_pasteboard() {
                    entries.push(string_item);
                }
                return Some(ClipboardItem { entries });
            }

            // Next, check for a plain string.
            if let Some(string_entry) = self.read_string_from_pasteboard() {
                return Some(ClipboardItem {
                    entries: vec![string_entry],
                });
            }

            // Finally, try the various supported image types.
            for format in ImageFormat::iter() {
                if let Some(item) = self.read_image(format) {
                    return Some(item);
                }
            }
        }

        None
    }

    /// File URLs are the current AppKit representation; retain legacy paths too.
    pub(crate) fn read_paths(&self) -> Option<ExternalPaths> {
        unsafe {
            let mut paths = SmallVec::new();
            let classes = NSArray::arrayWithObject(nil, class!(NSURL) as *const _ as id);
            let urls: id = msg_send![*self.inner, readObjectsForClasses: classes options: nil];
            if urls != nil {
                for url in urls.iter() {
                    let file: objc::runtime::BOOL = msg_send![url, isFileURL];
                    if file == objc::runtime::YES {
                        let path: id = msg_send![url, path];
                        if path != nil {
                            let value = CStr::from_ptr(NSString::UTF8String(path));
                            paths.push(PathBuf::from(value.to_string_lossy().into_owned()));
                        }
                    }
                }
            }
            if paths.is_empty() {
                let filenames =
                    NSPasteboard::propertyListForType(*self.inner, NSFilenamesPboardType);
                if filenames != nil {
                    for file in filenames.iter() {
                        let value = CStr::from_ptr(NSString::UTF8String(file));
                        paths.push(PathBuf::from(value.to_string_lossy().into_owned()));
                    }
                }
            }
            (!paths.is_empty()).then_some(ExternalPaths(paths))
        }
    }

    pub(crate) fn read_drag(
        &self,
        receiving: bool,
    ) -> Option<(ExternalPaths, Option<tempfile::TempDir>)> {
        if let Some(paths) = self.read_paths() {
            if receiving && paths.0.iter().any(|path| is_image_file(path)) {
                // AppKit may grant the receiving app access to Downloads or a
                // screenshot without granting the already-running CLI access.
                // Receive every image while the explicit drop grant is active.
                let image = stage_drag_image_files(&paths)
                    .or_else(|error| {
                        // Screenshot drags can expose image data even when the
                        // promised file URL has already become unavailable.
                        if paths.0.len() == 1 {
                            self.drag_image().ok_or(error)
                        } else {
                            Err(error)
                        }
                    })
                    .inspect_err(|error| log::warn!("Could not receive dragged images: {error}"))
                    .ok()?;
                return Some((image.0, Some(image.1)));
            }
            return Some((paths, None));
        }
        self.drag_image()
            .map(|(paths, directory)| (paths, Some(directory)))
    }

    /// Some image drags offer bytes instead of an existing path.
    /// Stage them only during an explicit drag, in a private temporary directory.
    pub(crate) fn drag_image(&self) -> Option<(ExternalPaths, tempfile::TempDir)> {
        for format in [ImageFormat::Png, ImageFormat::Tiff, ImageFormat::Jpeg] {
            if let Some(item) = self.read_image(format)
                && let Some(ClipboardEntry::Image(image)) = item.entries.first()
            {
                return stage_drag_image(&image.bytes).ok();
            }
        }
        None
    }

    fn read_image(&self, format: ImageFormat) -> Option<ClipboardItem> {
        let ut_type: UTType = format.into();

        unsafe {
            let types: id = self.inner.types();
            if msg_send![types, containsObject: ut_type.inner()] {
                self.data_for_type(ut_type.inner_mut()).map(|bytes| {
                    let bytes = bytes.to_vec();
                    let id = hash(&bytes);

                    ClipboardItem {
                        entries: vec![ClipboardEntry::Image(Image { format, bytes, id })],
                    }
                })
            } else {
                None
            }
        }
    }

    unsafe fn read_string_from_pasteboard(&self) -> Option<ClipboardEntry> {
        unsafe {
            let pasteboard_types: id = self.inner.types();
            let string_type: id = ns_string("public.utf8-plain-text");

            if !msg_send![pasteboard_types, containsObject: string_type] {
                return None;
            }

            let text_bytes = self.data_for_type(string_type)?;

            let text = String::from_utf8_lossy(&text_bytes).to_string();
            let metadata = self
                .data_for_type(*self.text_hash_type)
                .and_then(|hash_bytes| {
                    let hash_bytes = hash_bytes.as_slice().try_into().ok()?;
                    let hash = u64::from_be_bytes(hash_bytes);
                    let metadata = self.data_for_type(*self.metadata_type)?;

                    if hash == ClipboardString::text_hash(&text) {
                        String::from_utf8(metadata).ok()
                    } else {
                        None
                    }
                });

            Some(ClipboardEntry::String(ClipboardString { text, metadata }))
        }
    }

    unsafe fn data_for_type(&self, kind: id) -> Option<Vec<u8>> {
        unsafe {
            let data = self.inner.dataForType(kind);
            if data == nil {
                None
            } else if data.bytes().is_null() {
                Some(Vec::new())
            } else {
                Some(
                    slice::from_raw_parts(data.bytes() as *mut u8, data.length() as usize).to_vec(),
                )
            }
        }
    }

    pub fn write(&self, item: ClipboardItem) {
        unsafe {
            match item.entries.as_slice() {
                [] => {
                    // Writing an empty list of entries just clears the clipboard.
                    self.inner.clearContents();
                }
                [ClipboardEntry::String(string)] => {
                    self.write_plaintext(string);
                }
                [ClipboardEntry::Image(image)] => {
                    self.write_image(image);
                }
                [ClipboardEntry::ExternalPaths(_)] => {}
                _ => {
                    // Agus NB: We're currently only writing string entries to the clipboard when we have more than one.
                    //
                    // This was the existing behavior before I refactored the outer clipboard code:
                    // https://github.com/zed-industries/zed/blob/65f7412a0265552b06ce122655369d6cc7381dd6/crates/gpui/src/platform/mac/platform.rs#L1060-L1110
                    //
                    // Note how `any_images` is always `false`. We should fix that, but that's orthogonal to the refactor.

                    let mut combined = ClipboardString {
                        text: String::new(),
                        metadata: None,
                    };

                    for entry in item.entries {
                        match entry {
                            ClipboardEntry::String(text) => {
                                combined.text.push_str(&text.text());
                                if combined.metadata.is_none() {
                                    combined.metadata = text.metadata;
                                }
                            }
                            _ => {}
                        }
                    }

                    self.write_plaintext(&combined);
                }
            }
        }
    }

    fn write_plaintext(&self, string: &ClipboardString) {
        unsafe {
            self.inner.clearContents();

            let text_bytes = NSData::dataWithBytes_length_(
                nil,
                string.text.as_ptr() as *const c_void,
                string.text.len() as u64,
            );
            self.inner
                .setData_forType(text_bytes, NSPasteboardTypeString);

            if let Some(metadata) = string.metadata.as_ref() {
                let hash_bytes = ClipboardString::text_hash(&string.text).to_be_bytes();
                let hash_bytes = NSData::dataWithBytes_length_(
                    nil,
                    hash_bytes.as_ptr() as *const c_void,
                    hash_bytes.len() as u64,
                );
                self.inner.setData_forType(hash_bytes, *self.text_hash_type);

                let metadata_bytes = NSData::dataWithBytes_length_(
                    nil,
                    metadata.as_ptr() as *const c_void,
                    metadata.len() as u64,
                );
                self.inner
                    .setData_forType(metadata_bytes, *self.metadata_type);
            }
        }
    }

    unsafe fn write_image(&self, image: &Image) {
        unsafe {
            self.inner.clearContents();

            let bytes = NSData::dataWithBytes_length_(
                nil,
                image.bytes.as_ptr() as *const c_void,
                image.bytes.len() as u64,
            );

            self.inner
                .setData_forType(bytes, Into::<UTType>::into(image.format).inner_mut());
        }
    }
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    /// [Apple's documentation](https://developer.apple.com/documentation/appkit/nspasteboardnamefind?language=objc)
    pub static NSPasteboardNameFind: id;
}

impl From<ImageFormat> for UTType {
    fn from(value: ImageFormat) -> Self {
        match value {
            ImageFormat::Png => Self::png(),
            ImageFormat::Jpeg => Self::jpeg(),
            ImageFormat::Tiff => Self::tiff(),
            ImageFormat::Webp => Self::webp(),
            ImageFormat::Gif => Self::gif(),
            ImageFormat::Bmp => Self::bmp(),
            ImageFormat::Svg => Self::svg(),
            ImageFormat::Ico => Self::ico(),
            ImageFormat::Pnm => Self::pnm(),
        }
    }
}

// See https://developer.apple.com/documentation/uniformtypeidentifiers/uttype-swift.struct/
pub struct UTType(id);

impl UTType {
    pub fn png() -> Self {
        // https://developer.apple.com/documentation/uniformtypeidentifiers/uttype-swift.struct/png
        Self(unsafe { NSPasteboardTypePNG }) // This is a rare case where there's a built-in NSPasteboardType
    }

    pub fn jpeg() -> Self {
        // https://developer.apple.com/documentation/uniformtypeidentifiers/uttype-swift.struct/jpeg
        Self(unsafe { ns_string("public.jpeg") })
    }

    pub fn gif() -> Self {
        // https://developer.apple.com/documentation/uniformtypeidentifiers/uttype-swift.struct/gif
        Self(unsafe { ns_string("com.compuserve.gif") })
    }

    pub fn webp() -> Self {
        // https://developer.apple.com/documentation/uniformtypeidentifiers/uttype-swift.struct/webp
        Self(unsafe { ns_string("org.webmproject.webp") })
    }

    pub fn bmp() -> Self {
        // https://developer.apple.com/documentation/uniformtypeidentifiers/uttype-swift.struct/bmp
        Self(unsafe { ns_string("com.microsoft.bmp") })
    }

    pub fn svg() -> Self {
        // https://developer.apple.com/documentation/uniformtypeidentifiers/uttype-swift.struct/svg
        Self(unsafe { ns_string("public.svg-image") })
    }

    pub fn ico() -> Self {
        // https://developer.apple.com/documentation/uniformtypeidentifiers/uttype-swift.struct/ico
        Self(unsafe { ns_string("com.microsoft.ico") })
    }

    pub fn tiff() -> Self {
        // https://developer.apple.com/documentation/uniformtypeidentifiers/uttype-swift.struct/tiff
        Self(unsafe { NSPasteboardTypeTIFF }) // This is a rare case where there's a built-in NSPasteboardType
    }

    pub fn pnm() -> Self {
        //https://en.wikipedia.org/w/index.php?title=Netpbm&oldid=1336679433 under Uniform Type Identifier
        Self(unsafe { ns_string("public.pbm") })
    }

    fn inner(&self) -> *const Object {
        self.0
    }

    pub fn inner_mut(&self) -> *mut Object {
        self.0 as *mut _
    }
}

/// Keep a screenshot alive after a successful drop so the terminal program can
/// read it asynchronously. A cancelled drag drops the directory normally.
fn stage_drag_image(bytes: &[u8]) -> anyhow::Result<(ExternalPaths, tempfile::TempDir)> {
    anyhow::ensure!(bytes.len() <= 64 * 1024 * 1024, "dragged image is too large");
    let directory = tempfile::Builder::new()
        .prefix("agent-interface-drop-")
        .tempdir()?;
    let path = directory.path().join("Screenshot.png");
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    anyhow::ensure!(
        matches!(reader.format(), Some(image::ImageFormat::Png | image::ImageFormat::Tiff | image::ImageFormat::Jpeg)),
        "dragged image must be PNG, TIFF or JPEG"
    );
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    limits.reserve(decoder.total_bytes())?;
    if matches!(
        orientation,
        image::metadata::Orientation::Rotate90
            | image::metadata::Orientation::Rotate270
            | image::metadata::Orientation::Rotate90FlipH
            | image::metadata::Orientation::Rotate270FlipH
    ) {
        // Quarter turns allocate a second pixel buffer instead of working in place.
        limits.reserve(decoder.total_bytes())?;
    }
    decoder.set_limits(limits)?;
    let mut decoded = image::DynamicImage::from_decoder(decoder)?;
    decoded.apply_orientation(orientation);
    decoded.save_with_format(&path, image::ImageFormat::Png)?;
    Ok((ExternalPaths([path].into_iter().collect()), directory))
}

fn is_image_file(path: &std::path::Path) -> bool {
    matches!(
        image::ImageFormat::from_path(path),
        Ok(image::ImageFormat::Png | image::ImageFormat::Tiff | image::ImageFormat::Jpeg)
    )
}

/// Keep mixed drops ordered, staging images without altering ordinary paths.
fn stage_drag_image_files(paths: &ExternalPaths) -> anyhow::Result<(ExternalPaths, tempfile::TempDir)> {
    anyhow::ensure!(paths.0.len() <= 64, "too many dropped files");
    let directory = tempfile::Builder::new().prefix("agent-interface-drop-").tempdir()?;
    let mut result = Vec::with_capacity(paths.0.len());
    let mut total = 0;
    for (index, source) in paths.0.iter().enumerate() {
        if !is_image_file(source) {
            result.push(source.clone());
            continue;
        }
        total += std::fs::metadata(source)?.len();
        anyhow::ensure!(total <= 64 * 1024 * 1024, "dropped images are too large");
        let (staged, _temporary) = stage_drag_image_file(source)?;
        let path = directory.path().join(format!("Image-{}.png", index + 1));
        std::fs::rename(&staged.0[0], &path)?;
        result.push(path);
    }
    Ok((ExternalPaths(result.into()), directory))
}

fn stage_drag_image_file(
    path: &std::path::Path,
) -> anyhow::Result<(ExternalPaths, tempfile::TempDir)> {
    anyhow::ensure!(std::fs::metadata(path)?.is_file(), "dragged image is not a file");
    let file = std::fs::File::open(path)?;
    anyhow::ensure!(file.metadata()?.is_file(), "dragged image is not a file");
    let mut bytes = Vec::new();
    file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    stage_drag_image(&bytes)
}

#[cfg(test)]
mod tests {
    use cocoa::{
        appkit::{NSFilenamesPboardType, NSPasteboard, NSPasteboardTypeString},
        base::{id, nil},
        foundation::{NSArray, NSData},
    };
    use std::ffi::c_void;

    use gpui::{ClipboardEntry, ClipboardItem, ClipboardString, ImageFormat};
    use objc::rc::autoreleasepool;

    use super::*;

    #[test]
    fn drag_staging_rejects_formats_outside_the_advertised_types() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut bytes, image::ImageFormat::Bmp)
            .unwrap();
        assert!(stage_drag_image(bytes.get_ref()).unwrap_err().to_string().contains("PNG, TIFF or JPEG"));
    }

    #[test]
    fn screenshot_drag_stages_png_and_cleans_up_when_cancelled() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2, 3)
            .write_to(&mut bytes, image::ImageFormat::Tiff)
            .unwrap();
        let image = Image {
            format: ImageFormat::Tiff,
            bytes: bytes.into_inner(),
            id: 0,
        };
        let (paths, directory) = stage_drag_image(&image.bytes).unwrap();
        let path = paths.0[0].clone();
        assert_eq!(image::open(&path).unwrap().width(), 2);
        assert_eq!(path.extension().unwrap(), "png");
        drop(directory);
        assert!(!path.exists());
        let (paths, directory) = stage_drag_image(&image.bytes).unwrap();
        let kept = directory.keep();
        assert!(paths.0[0].exists());
        std::fs::remove_dir_all(kept).unwrap();
    }

    #[test]
    fn temporary_screenshot_drop_receives_an_independent_image() {
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("Screenshot at 3.09.04 PM.jpg");
        image::DynamicImage::new_rgb8(2, 3).save(&path).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        // EXIF orientation 6: the displayed image is rotated 90 degrees.
        bytes.splice(
            2..2,
            [
                0xff, 0xe1, 0, 34, b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0,
                1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
            ],
        );
        let mut oversized = bytes.clone();
        let frame = oversized.windows(2).position(|pair| pair == [0xff, 0xc0]).unwrap();
        // 8192x8192 RGB fits once, but rotation needs a second buffer.
        oversized[frame + 5..frame + 9].copy_from_slice(&[0x20, 0, 0x20, 0]);
        let error = stage_drag_image(&oversized).unwrap_err();
        assert!(matches!(error.downcast_ref::<image::ImageError>(), Some(image::ImageError::Limits(_))));
        std::fs::write(&path, bytes).unwrap();
        let pasteboard = Pasteboard::unique();
        unsafe { simulate_external_file_copy(&pasteboard, &[path.to_str().unwrap()]) };
        let (hover_paths, hover_directory) = pasteboard.read_drag(false).unwrap();
        assert_eq!(hover_paths.0[0], path);
        assert!(hover_directory.is_none());
        let (received, directory) = pasteboard.read_drag(true).unwrap();
        assert_ne!(received.0[0], path);
        assert!(path.exists());
        drop(source);
        assert_eq!(image::image_dimensions(&received.0[0]).unwrap(), (3, 2));
        drop(directory);
        assert!(!received.0[0].exists());
    }

    #[test]
    fn image_drops_from_any_folder_are_received_in_order_with_other_files() {
        let source = tempfile::tempdir().unwrap();
        let downloads = source.path().join("Downloads");
        let screenshots = source.path().join("Screenshots");
        std::fs::create_dir(&downloads).unwrap();
        std::fs::create_dir(&screenshots).unwrap();
        let first = downloads.join("photo with spaces.jpeg");
        let second = screenshots.join("photo.png");
        image::DynamicImage::new_rgb8(2, 3).save(&first).unwrap();
        image::DynamicImage::new_rgb8(4, 5).save(&second).unwrap();
        let ordinary = source.path().join("notes.txt");
        let paths = ExternalPaths(vec![first.clone(), ordinary.clone(), second.clone()].into());
        let (received, directory) = stage_drag_image_files(&paths).unwrap();
        assert_ne!(received.0[0], first);
        assert_eq!(received.0[1], ordinary);
        assert_ne!(received.0[2], second);
        drop(source);
        assert_eq!(image::image_dimensions(&received.0[0]).unwrap(), (2, 3));
        assert_eq!(image::image_dimensions(&received.0[2]).unwrap(), (4, 5));
        drop(directory);
        assert!(!received.0[0].exists());
        assert!(!received.0[2].exists());
    }

    unsafe fn simulate_external_file_copy(pasteboard: &Pasteboard, paths: &[&str]) {
        unsafe {
            let ns_paths: Vec<id> = paths.iter().map(|p| ns_string(p)).collect();
            let ns_array = NSArray::arrayWithObjects(nil, &ns_paths);

            let mut types = vec![NSFilenamesPboardType];
            types.push(NSPasteboardTypeString);

            let types_array = NSArray::arrayWithObjects(nil, &types);
            pasteboard.inner.declareTypes_owner(types_array, nil);

            pasteboard
                .inner
                .setPropertyList_forType(ns_array, NSFilenamesPboardType);

            let joined = paths.join("\n");
            let bytes = NSData::dataWithBytes_length_(
                nil,
                joined.as_ptr() as *const c_void,
                joined.len() as u64,
            );
            pasteboard
                .inner
                .setData_forType(bytes, NSPasteboardTypeString);
        }
    }

    #[test]
    fn test_string() {
        let pasteboard = Pasteboard::unique();
        assert_eq!(pasteboard.read(), None);

        let item = ClipboardItem::new_string("1".to_string());
        pasteboard.write(item.clone());
        assert_eq!(pasteboard.read(), Some(item));

        let item = ClipboardItem {
            entries: vec![ClipboardEntry::String(
                ClipboardString::new("2".to_string()).with_json_metadata(vec![3, 4]),
            )],
        };
        pasteboard.write(item.clone());
        assert_eq!(pasteboard.read(), Some(item));

        let text_from_other_app = "text from other app";
        unsafe {
            let bytes = NSData::dataWithBytes_length_(
                nil,
                text_from_other_app.as_ptr() as *const c_void,
                text_from_other_app.len() as u64,
            );
            pasteboard
                .inner
                .setData_forType(bytes, NSPasteboardTypeString);
        }
        assert_eq!(
            pasteboard.read(),
            Some(ClipboardItem::new_string(text_from_other_app.to_string()))
        );
    }

    #[test]
    fn test_custom_types_survive_creation_autorelease_pool() {
        let pasteboard = autoreleasepool(|| unsafe { Pasteboard::new(nil) });

        unsafe {
            let text_hash_type = CStr::from_ptr(NSString::UTF8String(*pasteboard.text_hash_type));
            let metadata_type = CStr::from_ptr(NSString::UTF8String(*pasteboard.metadata_type));
            assert_eq!(text_hash_type.to_bytes(), b"zed-text-hash");
            assert_eq!(metadata_type.to_bytes(), b"zed-metadata");
        }
    }

    #[test]
    fn test_read_external_path() {
        let pasteboard = Pasteboard::unique();

        unsafe {
            simulate_external_file_copy(&pasteboard, &["/test.txt"]);
        }

        let item = pasteboard.read().expect("should read clipboard item");

        // Test both ExternalPaths and String entries exist
        assert_eq!(item.entries.len(), 2);

        // Test first entry is ExternalPaths
        match &item.entries[0] {
            ClipboardEntry::ExternalPaths(ep) => {
                assert_eq!(ep.paths(), &[PathBuf::from("/test.txt")]);
            }
            other => panic!("expected ExternalPaths, got {:?}", other),
        }

        // Test second entry is String
        match &item.entries[1] {
            ClipboardEntry::String(s) => {
                assert_eq!(s.text(), "/test.txt");
            }
            other => panic!("expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_read_external_paths_with_spaces() {
        let pasteboard = Pasteboard::unique();
        let paths = ["/some file with spaces.txt"];

        unsafe {
            simulate_external_file_copy(&pasteboard, &paths);
        }

        let item = pasteboard.read().expect("should read clipboard item");

        match &item.entries[0] {
            ClipboardEntry::ExternalPaths(ep) => {
                assert_eq!(ep.paths(), &[PathBuf::from("/some file with spaces.txt")]);
            }
            other => panic!("expected ExternalPaths, got {:?}", other),
        }
    }

    #[test]
    fn test_read_multiple_external_paths() {
        let pasteboard = Pasteboard::unique();
        let paths = ["/file.txt", "/image.png"];

        unsafe {
            simulate_external_file_copy(&pasteboard, &paths);
        }

        let item = pasteboard.read().expect("should read clipboard item");
        assert_eq!(item.entries.len(), 2);

        // Test both ExternalPaths and String entries exist
        match &item.entries[0] {
            ClipboardEntry::ExternalPaths(ep) => {
                assert_eq!(
                    ep.paths(),
                    &[PathBuf::from("/file.txt"), PathBuf::from("/image.png"),]
                );
            }
            other => panic!("expected ExternalPaths, got {:?}", other),
        }

        match &item.entries[1] {
            ClipboardEntry::String(s) => {
                assert_eq!(s.text(), "/file.txt\n/image.png");
                assert_eq!(s.metadata, None);
            }
            other => panic!("expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_read_image() {
        let pasteboard = Pasteboard::unique();

        // Smallest valid PNG: 1x1 transparent pixel
        let png_bytes: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x62, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27, 0xDE, 0xFC, 0x00, 0x00,
            0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];

        unsafe {
            let ns_png_type = NSPasteboardTypePNG;
            let types_array = NSArray::arrayWithObjects(nil, &[ns_png_type]);
            pasteboard.inner.declareTypes_owner(types_array, nil);

            let data = NSData::dataWithBytes_length_(
                nil,
                png_bytes.as_ptr() as *const c_void,
                png_bytes.len() as u64,
            );
            pasteboard.inner.setData_forType(data, ns_png_type);
        }

        let item = pasteboard.read().expect("should read PNG image");

        // Test Image entry exists
        assert_eq!(item.entries.len(), 1);
        match &item.entries[0] {
            ClipboardEntry::Image(img) => {
                assert_eq!(img.format, ImageFormat::Png);
                assert_eq!(img.bytes, png_bytes);
            }
            other => panic!("expected Image, got {:?}", other),
        }
    }
}
