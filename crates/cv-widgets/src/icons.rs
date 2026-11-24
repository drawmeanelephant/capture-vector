//! The app's icon set, as data. Glyphs are from Lucide
//! (<https://lucide.dev>), ISC licence:
//!
//! > Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022
//! > as part of Feather (MIT). All other copyright (c) for Lucide are held
//! > by Lucide Contributors 2022.
//! >
//! > Permission to use, copy, modify, and/or distribute this software for
//! > any purpose with or without fee is hereby granted, provided that the
//! > above copyright notice and this permission notice appear in all
//! > copies.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL
//! > WARRANTIES WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED
//! > WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
//! > AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL
//! > DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR
//! > PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
//! > TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
//! > PERFORMANCE OF THIS SOFTWARE.
//!
//! Lucide draws 24-unit strokes from the root `<svg>`'s paint attributes;
//! each body wraps its elements in a group carrying those attributes.

use rune::Icon;

macro_rules! lucide {
    ($id:ident, $name:literal, $body:literal) => {
        pub static $id: Icon = Icon {
            name: concat!("cv-", $name),
            view: 24.0,
            body: concat!(
                r#"<g fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</g>"
            ),
        };
    };
}

lucide!(
    SUN,
    "sun",
    r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>"#
);
lucide!(
    MOON,
    "moon",
    r#"<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>"#
);
lucide!(
    ZAP,
    "zap",
    r#"<path d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z"/>"#
);
lucide!(
    ZAP_OFF,
    "zap-off",
    r#"<path d="M10.513 4.856 13.12 2.17a.5.5 0 0 1 .86.46l-1.377 4.317"/><path d="M15.656 10H20a1 1 0 0 1 .78 1.63l-1.72 1.773"/><path d="M16.273 16.273 10.88 21.83a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14H4a1 1 0 0 1-.78-1.63l4.507-4.643"/><path d="m2 2 20 20"/>"#
);
lucide!(
    TRIANGLE_ALERT,
    "triangle-alert",
    r#"<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>"#
);
lucide!(X, "x", r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_rasterizes() {
        for i in [&SUN, &MOON, &ZAP, &ZAP_OFF, &TRIANGLE_ALERT, &X] {
            let cov = rune::text::icon_coverage(i, 24);
            assert!(cov.iter().any(|&a| a > 0), "{} drew nothing", i.name);
        }
    }
}
