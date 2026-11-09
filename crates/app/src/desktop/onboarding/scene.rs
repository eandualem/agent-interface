//! The opening scene. The app's face laughs, looks right, looks left and then
//! winks into the icon, exactly as the website's hero does; its open amber eye
//! then flies to the person's first profile and becomes its mark.
//!
//! Every part is a pure function of time ([`Face::at`], [`Scene::at`]), so the
//! window redraws it each frame and tests can check any moment.

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    Bounds, BoxShadow, Corners, Div, PathBuilder, Pixels, Point, Rgba, Window, canvas, div, fill,
    linear_color_stop, linear_gradient, point, prelude::*, px, size,
};

use crate::desktop::{
    components as ui,
    design::{TextStyle, colors, tokens},
};

/// Where an element was laid out in the last frame, for the eye's flight.
pub(in crate::desktop) type Place = Rc<Cell<Option<Bounds<Pixels>>>>;

/// The scene's clock. It starts on the second frame the scene paints, once
/// the window is on screen, so a slow first frame (GPUI compiles its shaders
/// as the window opens) never eats into the face's laugh.
#[derive(Clone, Default)]
pub(super) struct Clock(Rc<Cell<(bool, Option<Instant>)>>);

impl Clock {
    /// How far the scene has run.
    pub fn ms(&self) -> f32 {
        self.0
            .get()
            .1
            .map_or(0.0, |start| start.elapsed().as_secs_f32() * 1000.0)
    }

    /// Move the scene to `ms`, as a skip does.
    pub fn set(&self, ms: f32) {
        if let Some(start) = Instant::now().checked_sub(Duration::from_secs_f32(ms / 1000.0)) {
            self.0.set((true, Some(start)));
        }
    }

    /// Count a painted frame; the second one starts the clock.
    fn tick(&self) {
        let (painted, start) = self.0.get();
        if start.is_none() {
            self.0.set((true, painted.then(Instant::now)));
        }
    }
}

// ---------------------------------------------------------------- the face

/// The face's own clock: the website's intro lasts five seconds and then rests
/// on the icon. Its keyframes below are given as percentages of it.
const FACE_MS: f32 = 5000.0;
/// The icon's drawing space: a 1024-unit square whose tile spans 100 to 924.
const ICON_ORIGIN: f32 = 100.0;
const ICON_SPAN: f32 = 824.0;
/// The icon's open eye: x, y, width, height and corner radius.
const EYE: [f32; 5] = [597.0, 365.0, 66.0, 150.0, 18.0];
/// The width of the icon's strokes: the closed eye and the mouth.
const STROKE: f32 = 35.0;

/// The left eye's top, middle and bottom points, its stroke width, and how far
/// it has closed: open is a short amber stroke, closed is the icon's white `>`.
const OPEN: [f32; 8] = [394.0, 396.0, 394.0, 440.0, 394.0, 484.0, 62.0, 0.0];
const CLOSED: [f32; 8] = [362.0, 384.0, 426.0, 440.0, 362.0, 496.0, STROKE, 1.0];
/// The mouth's start, control and end points.
const LAUGH: [f32; 6] = [630.0, 672.0, 521.0, 792.0, 412.0, 672.0];
const CHUCKLE: [f32; 6] = [626.0, 682.0, 521.0, 756.0, 416.0, 682.0];
const GRIN: [f32; 6] = [624.0, 682.0, 521.0, 768.0, 418.0, 682.0];
const LIFT: [f32; 6] = [622.0, 706.0, 536.0, 758.0, 420.0, 666.0];
const SMILE: [f32; 6] = [622.0, 700.0, 532.0, 752.0, 420.0, 676.0];

/// The website's keyframes: laugh with both eyes open (0–24 %), settle into a
/// grin, look right (28–44 %) and left (44–60 %), centre, then wink (64–72 %)
/// while the mouth lifts, overshoots and settles on the icon (72–78 %).
const LEFT: [(f32, [f32; 8]); 4] = [(0.0, OPEN), (64.0, OPEN), (72.0, CLOSED), (100.0, CLOSED)];
const LOOK: [(f32, [f32; 1]); 8] = [
    (0.0, [0.0]),
    (28.0, [0.0]),
    (34.0, [32.0]),
    (44.0, [32.0]),
    (52.0, [-32.0]),
    (60.0, [-32.0]),
    (66.0, [0.0]),
    (100.0, [0.0]),
];
const MOUTH: [(f32, [f32; 6]); 10] = [
    (0.0, LAUGH),
    (6.0, CHUCKLE),
    (12.0, LAUGH),
    (18.0, CHUCKLE),
    (24.0, LAUGH),
    (30.0, GRIN),
    (64.0, GRIN),
    (72.0, LIFT),
    (78.0, SMILE),
    (100.0, SMILE),
];
/// The laugh's bob, which moves the eyes and the mouth together.
const BOB: [(f32, [f32; 1]); 10] = [
    (0.0, [0.0]),
    (3.0, [-14.0]),
    (6.0, [0.0]),
    (9.0, [-14.0]),
    (12.0, [0.0]),
    (15.0, [-14.0]),
    (18.0, [0.0]),
    (21.0, [-14.0]),
    (24.0, [0.0]),
    (100.0, [0.0]),
];

/// CSS `cubic-bezier(x1, y1, x2, y2)` at progress `x`, so the face eases the
/// way it does on the website.
fn cubic_bezier((x1, y1): (f32, f32), (x2, y2): (f32, f32), x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let curve = |a: f32, b: f32, t: f32| {
        let u = 1.0 - t;
        3.0 * a * t * u * u + 3.0 * b * t * t * u + t * t * t
    };
    // With both control points inside 0..1 the x curve rises monotonically,
    // so bisection finds its parameter.
    let (mut low, mut high) = (0.0_f32, 1.0_f32);
    for _ in 0..32 {
        let mid = (low + high) / 2.0;
        if curve(x1, x2, mid) < x {
            low = mid;
        } else {
            high = mid;
        }
    }
    curve(y1, y2, (low + high) / 2.0)
}

/// The website face's curve.
fn face_ease(x: f32) -> f32 {
    cubic_bezier((0.45, 0.0), (0.2, 1.0), x)
}

/// CSS `ease-in-out`, which the website uses for the laugh's bob.
fn bob_ease(x: f32) -> f32 {
    cubic_bezier((0.42, 0.0), (0.58, 1.0), x)
}

/// A keyframe track at `percent`, easing each segment on its own as CSS does.
fn track<const N: usize>(
    stops: &[(f32, [f32; N])],
    percent: f32,
    ease: fn(f32) -> f32,
) -> [f32; N] {
    let mut previous = stops[0];
    if percent <= previous.0 {
        return previous.1;
    }
    for &(stop, value) in &stops[1..] {
        if percent <= stop {
            let t = ease((percent - previous.0) / (stop - previous.0));
            return std::array::from_fn(|i| previous.1[i] + (value[i] - previous.1[i]) * t);
        }
        previous = (stop, value);
    }
    previous.1
}

/// The face at one moment, in the icon's 1024-unit space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Face {
    /// The left eye's top, middle and bottom points.
    left: [(f32, f32); 3],
    left_width: f32,
    /// How far the left eye has closed: 0 open and amber, 1 the white `>`.
    wink: f32,
    /// Both eyes' sideways look.
    look: f32,
    /// The laugh's bob, applied to the eyes and the mouth.
    bob: f32,
    /// The mouth's start, control and end points.
    mouth: [(f32, f32); 3],
}

impl Face {
    /// The face `ms` milliseconds into the website's intro. Before it starts
    /// it laughs with both eyes open; after it ends it is the icon.
    pub fn at(ms: f32) -> Self {
        let percent = (ms / FACE_MS).clamp(0.0, 1.0) * 100.0;
        let left = track(&LEFT, percent, face_ease);
        let mouth = track(&MOUTH, percent, face_ease);
        Self {
            left: [(left[0], left[1]), (left[2], left[3]), (left[4], left[5])],
            left_width: left[6],
            wink: left[7],
            look: track(&LOOK, percent, face_ease)[0],
            bob: track(&BOB, percent, bob_ease)[0],
            mouth: [
                (mouth[0], mouth[1]),
                (mouth[2], mouth[3]),
                (mouth[4], mouth[5]),
            ],
        }
    }

    /// The face at rest: the app icon.
    pub fn icon() -> Self {
        Self::at(FACE_MS)
    }
}

// ---------------------------------------------------------------- the scene

/// How long the tile and the words take to rise into place.
const RISE_MS: f32 = 600.0;
/// The line under the name follows it by this much.
const LINE_AFTER_MS: f32 = 200.0;
/// How long Skip and the interface size take to appear.
const CONTROLS_MS: f32 = 400.0;
/// How long the face and words take to leave once setup takes over.
const LEAVE_MS: f32 = 400.0;
/// Setup appears this long after the handoff, so the words have mostly gone.
const SETUP_AFTER_MS: f32 = 250.0;
const SETUP_MS: f32 = 500.0;
/// The first step's numeral appears once the eye has landed.
const NUMERAL_MS: f32 = 150.0;
/// How far the tile and the words travel as they rise and as they leave.
const RISE_PX: f32 = 8.0;
const WORDS_PX: f32 = 6.0;

/// What the scene shows at one moment; each part runs from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Scene {
    /// The tile rising into place.
    pub tile: f32,
    /// The face and the words leaving for setup.
    pub leave: f32,
    pub face: Face,
    pub name: f32,
    pub line: f32,
    /// Skip and the interface size.
    pub controls: f32,
    /// The open eye on its way from the face to the first step.
    pub flight: f32,
    /// Setup's rail, page and footer.
    pub setup: f32,
    /// The "1" inside the first step's marker.
    pub numeral: f32,
}

impl Scene {
    /// The scene `ms` milliseconds after it started.
    pub fn at(ms: f32) -> Self {
        let m = &tokens().motion;
        let rise = |from: f32, length: f32| ((ms - from) / length).clamp(0.0, 1.0);
        let out = gpui::ease_out_quint();
        let handoff = m.scene_handoff_ms;
        Self {
            tile: out(rise(0.0, RISE_MS)),
            leave: gpui::ease_in_out(rise(handoff, LEAVE_MS)),
            // Skipping lands on the handoff, where the face is already the icon.
            face: if ms >= handoff {
                Face::icon()
            } else {
                Face::at(ms - m.scene_face_ms)
            },
            name: out(rise(m.scene_words_ms, RISE_MS)),
            line: out(rise(m.scene_words_ms + LINE_AFTER_MS, RISE_MS)),
            controls: out(rise(m.scene_face_ms, CONTROLS_MS)),
            flight: gpui::ease_in_out(rise(handoff, m.scene_flight_ms)),
            setup: out(rise(handoff + SETUP_AFTER_MS, SETUP_MS)),
            numeral: rise(handoff + m.scene_flight_ms, NUMERAL_MS),
        }
    }

    /// Whether the open eye has landed and the first step's marker shows.
    pub fn landed(&self) -> bool {
        self.flight >= 1.0
    }

    /// The offset of setup's page as it settles.
    pub fn page_offset(&self) -> f32 {
        WORDS_PX * (1.0 - self.setup)
    }
}

/// Where the scene is after a skip: the handoff, unless it is already past it.
pub(super) fn skipped(ms: f32) -> f32 {
    ms.max(tokens().motion.scene_handoff_ms)
}

// ---------------------------------------------------------------- drawing

fn mix(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    Rgba {
        r: lerp(from.r, to.r),
        g: lerp(from.g, to.g),
        b: lerp(from.b, to.b),
        a: lerp(from.a, to.a),
    }
}

/// The icon-space rectangle of the open eye, as window bounds inside `tile`.
fn eye_bounds(tile: Bounds<Pixels>, face: &Face) -> Bounds<Pixels> {
    let scale = f32::from(tile.size.width) / ICON_SPAN;
    let [x, y, w, h, _] = EYE;
    Bounds::new(
        tile.origin
            + point(
                px((x + face.look - ICON_ORIGIN) * scale),
                px((y + face.bob - ICON_ORIGIN) * scale),
            ),
        size(px(w * scale), px(h * scale)),
    )
}

/// A round dot, which gives the strokes their round ends and joins.
fn dot(window: &mut Window, center: Point<Pixels>, width: f32, color: Rgba) {
    let radius = px(width / 2.0);
    window.paint_quad(
        fill(
            Bounds::new(center - point(radius, radius), size(px(width), px(width))),
            color,
        )
        .corner_radii(radius),
    );
}

fn segment(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, width: f32, color: Rgba) {
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(from);
    path.line_to(to);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// Paint the face into its tile. The open eye is left out while it flies.
fn paint_face(tile: Bounds<Pixels>, face: &Face, with_eye: bool, window: &mut Window) {
    let c = colors();
    let scale = f32::from(tile.size.width) / ICON_SPAN;
    let at = |(x, y): (f32, f32), look: f32| {
        tile.origin
            + point(
                px((x + look - ICON_ORIGIN) * scale),
                px((y + face.bob - ICON_ORIGIN) * scale),
            )
    };
    // The left eye: two segments with round ends and a round join, so the
    // short open stroke can fold into the `>` as it winks.
    let width = face.left_width * scale;
    let color = mix(c.mark_eye, c.mark_glyph, face.wink);
    let [top, middle, bottom] = face.left.map(|p| at(p, face.look));
    segment(window, top, middle, width, color);
    segment(window, middle, bottom, width, color);
    for center in [top, middle, bottom] {
        dot(window, center, width, color);
    }
    if with_eye {
        let eye = eye_bounds(tile, face);
        let radius = px(EYE[4] * scale);
        window.paint_drop_shadows(eye, Corners::all(radius), &[glow(scale, 1.0)]);
        window.paint_quad(fill(eye, c.mark_eye).corner_radii(radius));
    }
    // The mouth, a quadratic curve with round ends.
    let width = STROKE * scale;
    let [start, control, end] = face.mouth.map(|p| at(p, 0.0));
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(start);
    path.curve_to(end, control);
    if let Ok(path) = path.build() {
        window.paint_path(path, c.mark_glyph);
    }
    dot(window, start, width, c.mark_glyph);
    dot(window, end, width, c.mark_glyph);
}

/// The cursor eye's glow, as the icon draws it, faded by `strength`.
fn glow(scale: f32, strength: f32) -> BoxShadow {
    let mut color = colors().mark_glow;
    color.a *= strength;
    BoxShadow {
        color: color.into(),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(36.0 * scale),
        spread_radius: px(0.0),
        inset: false,
    }
}

/// The open eye between its place in the face and the profile's mark: it
/// takes the mark's shape and the theme's accent, and loses its glow.
fn paint_flight(from: Bounds<Pixels>, to: Bounds<Pixels>, progress: f32, window: &mut Window) {
    let c = colors();
    let lerp = |a: Pixels, b: Pixels, t: f32| px(f32::from(a) + (f32::from(b) - f32::from(a)) * t);
    // Dropping a little ahead of crossing makes the path a gentle arc.
    let down = (progress * 1.12).min(1.0);
    let bounds = Bounds::new(
        point(
            lerp(from.origin.x, to.origin.x, progress),
            lerp(from.origin.y, to.origin.y, down),
        ),
        size(
            lerp(from.size.width, to.size.width, progress),
            lerp(from.size.height, to.size.height, progress),
        ),
    );
    let scale = f32::from(from.size.width) / EYE[2];
    let radius = lerp(
        px(EYE[4] * scale),
        px(f32::from(to.size.width) * ui::PROFILE_MARK_ROUND),
        progress,
    );
    window.paint_drop_shadows(bounds, Corners::all(radius), &[glow(scale, 1.0 - progress)]);
    window.paint_quad(fill(bounds, mix(c.mark_eye, c.accent, progress)).corner_radii(radius));
}

/// The face, its name and line, and the flying eye, laid over setup. The
/// caller adds the click that skips. Painting it runs `clock`; `tile` records
/// where the face was laid out; `marker`, when the page has one, is where the
/// eye lands. Without it the eye stays in the face as the face leaves.
pub(super) fn overlay(
    scene: &Scene,
    clock: Clock,
    tile: Place,
    marker: Option<Place>,
    tagline: &'static str,
) -> Div {
    let (t, c) = (tokens(), colors());
    let mark = t.size.scene_mark;
    // The tile grows from 94 % as it rises, around its centre.
    let grown = mark * (0.94 + 0.06 * scene.tile);
    let inset = (mark - grown) / 2.0;
    let drift = -RISE_PX * scene.leave;
    let face = scene.face;
    let lands = marker.as_ref().is_some_and(|marker| marker.get().is_some());
    let flight = if lands { scene.flight } else { 0.0 };
    let laid_out = tile.clone();
    let tile_element = div()
        .absolute()
        .left(px(inset))
        .top(px(inset + RISE_PX * (1.0 - scene.tile) + drift))
        .size(px(grown))
        .rounded(px(grown * 186.0 / ICON_SPAN))
        .bg(linear_gradient(
            180.0,
            linear_color_stop(c.mark_from, 0.0),
            linear_color_stop(c.mark_to, 1.0),
        ))
        .border(px(t.size.hairline * 1.5))
        .border_color(c.mark_edge)
        .shadow(vec![BoxShadow {
            color: c.shadow.into(),
            offset: point(px(0.0), px(t.space.s16)),
            blur_radius: px(t.space.s16 * 2.0 + t.space.s6),
            spread_radius: px(0.0),
            inset: false,
        }])
        .opacity(scene.tile * (1.0 - scene.leave))
        .child(
            canvas(
                move |bounds, _, _| laid_out.set(Some(bounds)),
                move |bounds, (), window, _| paint_face(bounds, &face, flight <= 0.0, window),
            )
            .size_full(),
        );
    let words = |text: &'static str, style: &TextStyle, color: Rgba, shown: f32| {
        ui::text(div().relative(), style)
            .top(px(WORDS_PX * (1.0 - shown) + drift))
            .opacity(shown * (1.0 - scene.leave))
            .text_color(color)
            .whitespace_nowrap()
            .child(text)
    };
    div()
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .occlude()
        .child(
            div()
                .relative()
                .flex_shrink_0()
                .size(px(mark))
                .child(tile_element),
        )
        .child(div().h(px(t.space.s24)))
        .child(words(
            "Agent Interface",
            &t.text.scene_title,
            c.text,
            scene.name,
        ))
        .child(words(tagline, &t.text.scene_line, c.muted, scene.line))
        .child(
            canvas(
                |_, _, _| (),
                move |_, (), window, _| {
                    // Keep the scene moving until it hands over to setup.
                    clock.tick();
                    window.request_animation_frame();
                    if flight <= 0.0 || flight >= 1.0 {
                        return;
                    }
                    if let (Some(tile), Some(target)) =
                        (tile.get(), marker.as_ref().and_then(|marker| marker.get()))
                    {
                        paint_flight(eye_bounds(tile, &face), target, flight, window);
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
}

/// Records where the first step's marker is laid out, for the eye to land on.
pub(in crate::desktop) fn marker_place(marker: Place) -> impl IntoElement {
    canvas(
        move |bounds, _, _| marker.set(Some(bounds)),
        |_, (), _, _| (),
    )
    .absolute()
    .inset_0()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The icon's geometry, read from the app icon itself.
    fn icon_numbers(svg: &str, after: &str, attribute: &str) -> Vec<f32> {
        let tag = &svg[svg.find(after).expect("icon element")..];
        let start = tag
            .find(&format!("{attribute}=\""))
            .expect("icon attribute")
            + attribute.len()
            + 2;
        let value = &tag[start..start + tag[start..].find('"').unwrap()];
        value
            .split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .filter(|part| !part.is_empty())
            .map(|part| part.parse().unwrap())
            .collect()
    }

    #[test]
    fn the_face_starts_laughing_with_both_eyes_open() {
        let face = Face::at(0.0);
        assert_eq!(face.wink, 0.0);
        assert_eq!(face.left_width, 62.0);
        assert_eq!(face.left, [(394.0, 396.0), (394.0, 440.0), (394.0, 484.0)]);
        assert_eq!(face.mouth, [(630.0, 672.0), (521.0, 792.0), (412.0, 672.0)]);
        // The laugh bobs the features before the eyes ever move.
        assert!(Face::at(150.0).bob < -1.0);
        assert_eq!(Face::at(150.0).look, 0.0);
    }

    #[test]
    fn the_face_looks_right_then_left_and_winks_last() {
        let right = Face::at(0.40 * FACE_MS);
        let left = Face::at(0.56 * FACE_MS);
        assert_eq!((right.look, right.wink), (32.0, 0.0));
        assert_eq!((left.look, left.wink), (-32.0, 0.0));
        // Mid-wink, the eyes are centred and the left eye is folding.
        let winking = Face::at(0.68 * FACE_MS);
        assert_eq!(winking.look, 0.0);
        assert!(winking.wink > 0.0 && winking.wink < 1.0);
        assert!(winking.left[1].0 > 394.0 && winking.left[1].0 < 426.0);
    }

    #[test]
    fn the_face_comes_to_rest_on_the_app_icon() {
        let svg = include_str!("../../../../../packaging/icon/agent-interface.svg");
        let face = Face::icon();
        assert_eq!(face.wink, 1.0);
        assert_eq!((face.look, face.bob), (0.0, 0.0));
        let closed = icon_numbers(svg, "<polyline", "points");
        let left: Vec<f32> = face.left.iter().flat_map(|(x, y)| [*x, *y]).collect();
        assert_eq!(left, closed);
        assert_eq!(
            face.left_width,
            icon_numbers(svg, "<polyline", "stroke-width")[0]
        );
        let smile = icon_numbers(svg, "<path d=", "d");
        let mouth: Vec<f32> = face.mouth.iter().flat_map(|(x, y)| [*x, *y]).collect();
        assert_eq!(mouth, smile);
        // The cursor eye is the icon's only amber-filled rectangle.
        let fill = svg.find("fill=\"#e8a85f\"").expect("the cursor eye");
        let rect = &svg[svg[..fill].rfind("<rect").unwrap()..];
        for (attribute, value) in [
            ("x", EYE[0]),
            ("y", EYE[1]),
            ("width", EYE[2]),
            ("height", EYE[3]),
            ("rx", EYE[4]),
        ] {
            assert_eq!(
                icon_numbers(rect, "<rect", attribute)[0],
                value,
                "{attribute}"
            );
        }
    }

    #[test]
    fn the_face_eases_like_the_website() {
        assert_eq!(face_ease(0.0), 0.0);
        assert_eq!(face_ease(1.0), 1.0);
        // ease-in-out is symmetric; the face's curve leaves slowly and lands softly.
        assert!((bob_ease(0.5) - 0.5).abs() < 1e-4);
        assert!(face_ease(0.2) < 0.2 && face_ease(0.8) > 0.9);
        let mut previous = 0.0;
        for step in 1..=100 {
            let eased = face_ease(step as f32 / 100.0);
            assert!(eased >= previous);
            previous = eased;
        }
    }

    #[test]
    fn the_scene_hands_over_to_setup_through_the_first_step() {
        let m = &tokens().motion;
        let start = Scene::at(0.0);
        assert_eq!((start.tile, start.setup, start.flight), (0.0, 0.0, 0.0));
        let looking = Scene::at(m.scene_words_ms + RISE_MS + LINE_AFTER_MS);
        assert_eq!((looking.name, looking.line, looking.leave), (1.0, 1.0, 0.0));
        let winked = Scene::at(m.scene_handoff_ms - 1.0);
        assert_eq!(winked.face, Face::icon());
        assert_eq!(winked.setup, 0.0);
        let end = Scene::at(m.scene_ms);
        assert!(end.landed());
        assert_eq!((end.leave, end.setup, end.numeral), (1.0, 1.0, 1.0));
        assert!(m.scene_handoff_ms + m.scene_flight_ms + NUMERAL_MS <= m.scene_ms);
    }

    #[test]
    fn the_clock_waits_for_the_window_then_skips_forward() {
        let clock = Clock::default();
        clock.tick();
        // The first frame may not be on screen yet, so the face holds still.
        assert!(clock.0.get().1.is_none());
        assert_eq!(clock.ms(), 0.0);
        clock.tick();
        assert!(clock.0.get().1.is_some());
        let handoff = tokens().motion.scene_handoff_ms;
        clock.set(skipped(clock.ms()));
        assert!(clock.ms() >= handoff);
    }

    #[test]
    fn skipping_jumps_to_the_handoff_and_never_back() {
        let handoff = tokens().motion.scene_handoff_ms;
        assert_eq!(skipped(900.0), handoff);
        assert_eq!(skipped(handoff + 100.0), handoff + 100.0);
        // A skip mid-laugh shows the icon, ready to hand over.
        assert_eq!(Scene::at(skipped(900.0)).face, Face::icon());
    }
}
