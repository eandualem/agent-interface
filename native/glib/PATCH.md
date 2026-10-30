# GLib 0.18.5 compatibility patch

This is the MIT-licensed crates.io `glib` 0.18.5 source, with its licence retained.
Wry's GTK3/WebKitGTK bindings require the 0.18 API. The only implementation
change backports [gtk-rs-core#1343](https://github.com/gtk-rs/gtk-rs-core/pull/1343):
`VariantStrIter::impl_get` passes a mutable output pointer to GLib, fixing
[RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429).

The workspace patches crates.io to this directory. Do not replace this source
with unpatched 0.18.5. The optimized `variant_string_iterator` test in
`webview-host` exercises forward and reverse string-array iteration on Linux.
Remove this patch when the GTK/WebKit bindings can use fixed upstream GLib.
