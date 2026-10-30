//! UI models that are independent of the editor, port of `core/uimodel.js`: accessible candidate rows (UT-057),
//! overlay placement (UT-055) and the environment check (UT-077). Meaning is always carried by text (source,
//! validation, position), never by colour.
//!
//! The status bar and the menu of the extension are not here: the reference keeps them inside `extension.js` (they
//! are VS Code objects assembled from `i18n` texts, with no logic beyond choosing a key), so there is nothing in
//! `uimodel.js` to port for them.

use crate::i18n;
use crate::json::Json;
use crate::jsval::{number_to_string, round, to_js_string, truthy};

/// One row of the candidate list: name, position, source, validation and the available action, all as text.
/// `c` is the candidate as the bridge holds it (`label`, `source`/`sources`, `validation`, `explain`,
/// `assumptions`, `text`); a source or validation that is missing reads as the text `undefined`, as in the reference.
pub fn candidate_row(locale: &str, c: &Json, index: f64, total: f64) -> Json {
    let sources: Vec<Option<&Json>> = match c.get("sources") {
        Some(Json::Arr(list)) if !list.is_empty() => list.iter().map(Some).collect(),
        _ => vec![c.get("source")],
    };
    let word = |v: Option<&Json>| v.map_or_else(|| "undefined".to_string(), to_js_string);
    let first = i18n::text(locale, &format!("src.{}", word(sources[0])), &[]);
    let source = if sources.len() > 1 {
        format!("{first} +{}", sources.len() - 1)
    } else {
        first
    };
    let validation = i18n::text(locale, &format!("val.{}", word(c.get("validation"))), &[]);
    let assumptions: &[Json] = match c.get("assumptions") {
        Some(Json::Arr(a)) => a,
        _ => &[],
    };
    let detail = c
        .get("explain")
        .into_iter()
        .chain(assumptions)
        .filter(|v| truthy(v))
        .map(to_js_string)
        .collect::<Vec<_>>()
        .join(" · ");
    let label = c.get("label").map(to_js_string).unwrap_or_default();
    let insertable = !matches!(c.get("text"), None | Some(Json::Null));
    Json::obj(vec![
        ("label", Json::Str(label.clone())),
        ("description", Json::Str(format!("[{source}] {validation}"))),
        ("detail", Json::Str(detail)),
        (
            "ariaLabel",
            Json::Str(format!(
                "{} / {}: {label}, {source}, {validation}{}",
                number_to_string(index),
                number_to_string(total),
                if insertable { "" } else { ", not insertable" }
            )),
        ),
        ("insertable", Json::Bool(insertable)),
    ])
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Places a candidate window inside the monitor work area that contains the caret. Negative coordinates (monitors
/// left of / above the primary) are legal. `None` when the caret is on no monitor or the window does not fit: hide,
/// never steal focus. `dpi` is the display scaling in percent (clamped to 100-250, default 100).
pub fn place_overlay(
    caret: (f64, f64),
    size: (f64, f64),
    monitors: &[Rect],
    dpi: f64,
) -> Option<Rect> {
    let scale = dpi.clamp(100.0, 250.0) / 100.0;
    let (w, h) = (round(size.0 * scale), round(size.1 * scale));
    let (cx, cy) = caret;
    let mon = monitors
        .iter()
        .find(|m| cx >= m.x && cx < m.x + m.w && cy >= m.y && cy < m.y + m.h)?;
    if w > mon.w || h > mon.h {
        return None;
    }
    let (mut x, mut y) = (cx, cy + round(18.0 * scale)); // below the caret line
    if y + h > mon.y + mon.h {
        y = cy - h - 2.0; // not enough room below: flip above
    }
    if x + w > mon.x + mon.w {
        x = mon.x + mon.w - w; // clamp the right edge
    }
    Some(Rect {
        x: x.max(mon.x),
        y: y.max(mon.y),
        w,
        h,
    })
}

/// What the environment check looks at. A field that is `None` is "not given"; `dpi` given as anything but a finite
/// number counts as invalid (as `dpi: null` does in the reference), and a `dpi` that is not given is skipped.
pub struct Environment<'a> {
    pub platform: Option<&'a Json>,
    pub arch: Option<&'a Json>,
    pub build: Option<&'a Json>,
    pub dpi: Option<&'a Json>,
}

/// Supported: Windows 11 (build >= 22000) on x64, display scaling 100-250 %. Anything else is reported, not guessed.
/// Returns whether the environment is supported and the reasons it is not.
pub fn check_environment(env: &Environment) -> (bool, Vec<&'static str>) {
    let mut reasons = Vec::new();
    if env.platform.and_then(Json::as_str) != Some("win32") {
        reasons.push("Windows is required");
    }
    if env.arch.and_then(Json::as_str) != Some("x64") {
        reasons.push("x64 is required");
    }
    let build_ok = matches!(env.build, Some(Json::Num(n)) if n.is_finite() && n.fract() == 0.0 && *n >= 22000.0);
    if !build_ok {
        reasons.push("Windows 11 (build 22000 or newer) is required");
    }
    if let Some(dpi) = env.dpi {
        let ok = matches!(dpi, Json::Num(n) if n.is_finite() && (100.0..=250.0).contains(n));
        if !ok {
            reasons.push("display scaling must be between 100% and 250%");
        }
    }
    (reasons.is_empty(), reasons)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Limits, parse};

    fn j(s: &str) -> Json {
        parse(s, Limits::DEFAULT).unwrap()
    }

    const MONITORS: [Rect; 2] = [
        Rect {
            x: -1920.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        },
        Rect {
            x: 0.0,
            y: 0.0,
            w: 2560.0,
            h: 1440.0,
        },
    ];

    #[test]
    fn ut_057_every_candidate_row_is_described_by_text() {
        let cands = [
            j(
                r#"{"label":"main function","source":"rule","validation":"valid","text":"fn main() {}","explain":"Entry","assumptions":["a","","b"]}"#,
            ),
            j(r#"{"label":"AI idea","source":"ai","validation":"unchecked","text":"x"}"#),
            j(r#"{"label":"jmp foo","source":"rule","validation":"context_missing","text":null}"#),
        ];
        for locale in i18n::LOCALES {
            for (i, c) in cands.iter().enumerate() {
                let row = candidate_row(locale, c, (i + 1) as f64, 3.0);
                let aria = row.get("ariaLabel").and_then(Json::as_str).unwrap();
                assert!(aria.starts_with(&format!("{} / 3:", i + 1)), "{aria}");
                let desc = row.get("description").and_then(Json::as_str).unwrap();
                assert!(desc.contains(&i18n::text(
                    locale,
                    &format!("src.{}", c.get("source").and_then(Json::as_str).unwrap()),
                    &[]
                )));
                assert!(desc.contains(&i18n::text(
                    locale,
                    &format!(
                        "val.{}",
                        c.get("validation").and_then(Json::as_str).unwrap()
                    ),
                    &[]
                )));
            }
        }
        let row = candidate_row("en-US", &cands[2], 3.0, 3.0);
        assert_eq!(row.get("insertable"), Some(&Json::Bool(false)));
        assert!(
            row.get("ariaLabel")
                .and_then(Json::as_str)
                .unwrap()
                .ends_with(", not insertable")
        );
        assert_eq!(
            candidate_row("en-US", &cands[0], 1.0, 3.0)
                .get("detail")
                .and_then(Json::as_str),
            Some("Entry · a · b"),
            "blank assumptions are dropped"
        );
        let merged = j(
            r#"{"label":"m","source":"rule","sources":["rule","ai"],"validation":"valid","text":""}"#,
        );
        let row = candidate_row("en-US", &merged, 1.0, 1.0);
        assert!(
            row.get("description")
                .and_then(Json::as_str)
                .unwrap()
                .contains("+1")
        );
        assert_eq!(
            row.get("insertable"),
            Some(&Json::Bool(true)),
            "an empty text is still text"
        );
        let bare = candidate_row("en-US", &j(r#"{"label":"x"}"#), 1.0, 1.0);
        assert_eq!(
            bare.get("description").and_then(Json::as_str),
            Some("[src.undefined] val.undefined")
        );
    }

    #[test]
    fn ut_055_the_overlay_stays_on_the_caret_monitor_and_hides_when_it_cannot_be_located() {
        let size = (320.0, 200.0);
        let edge = place_overlay((-30.0, 500.0), size, &MONITORS, 100.0).unwrap();
        assert!(
            edge.x >= -1920.0 && edge.x + edge.w <= 0.0,
            "clamped inside the left monitor"
        );
        let bottom = place_overlay((100.0, 1430.0), size, &MONITORS, 250.0).unwrap();
        assert!(
            bottom.y + bottom.h <= 1440.0 && bottom.y < 1430.0,
            "flips above the caret at the bottom edge"
        );
        assert_eq!(
            (bottom.w, bottom.h),
            (800.0, 500.0),
            "250% DPI scales the window"
        );
        assert_eq!(
            place_overlay((99999.0, 5.0), size, &MONITORS, 100.0),
            None,
            "caret on no monitor"
        );
        assert_eq!(
            place_overlay((10.0, 10.0), (4000.0, 10.0), &MONITORS, 100.0),
            None,
            "larger than the monitor"
        );
        // scaling is clamped to 100-250 and halves round up (Math.round)
        let r = place_overlay((10.0, 10.0), (2.0, 2.0), &MONITORS, 125.0).unwrap();
        assert_eq!((r.w, r.h, r.y), (3.0, 3.0, 33.0));
        assert_eq!(
            place_overlay((10.0, 10.0), size, &MONITORS, 10.0)
                .unwrap()
                .w,
            320.0
        );
        assert_eq!(
            place_overlay((10.0, 10.0), size, &MONITORS, 900.0)
                .unwrap()
                .w,
            800.0
        );
        // the right edge is clamped, the left edge of the monitor wins last
        let r = place_overlay((2500.0, 10.0), size, &MONITORS, 100.0).unwrap();
        assert_eq!(r.x, 2240.0);
    }

    #[test]
    fn ut_077_the_environment_check_explains_what_is_unsupported() {
        let check = |text: &str| {
            let v = j(text);
            let env = Environment {
                platform: v.get("platform"),
                arch: v.get("arch"),
                build: v.get("build"),
                dpi: v.get("dpi"),
            };
            check_environment(&env)
        };
        assert_eq!(
            check(r#"{"platform":"win32","arch":"x64","build":22631,"dpi":150}"#),
            (true, vec![])
        );
        for dpi in [100, 150, 200, 250] {
            assert!(
                check(&format!(
                    r#"{{"platform":"win32","arch":"x64","build":26100,"dpi":{dpi}}}"#
                ))
                .0
            );
        }
        assert!(!check(r#"{"platform":"win32","arch":"x64","build":19045}"#).0);
        assert_eq!(
            check(r#"{"platform":"win32","arch":"arm64","build":26100}"#).1,
            ["x64 is required"]
        );
        assert_eq!(
            check(r#"{"platform":"linux","arch":"x64","build":0}"#)
                .1
                .len(),
            2
        );
        assert!(!check(r#"{"platform":"win32","arch":"x64","build":26100,"dpi":300}"#).0);
        assert!(
            check(r#"{"platform":"win32","arch":"x64","build":26100}"#).0,
            "unknown dpi is skipped, not guessed"
        );
        assert!(
            !check(r#"{"platform":"win32","arch":"x64","build":26100,"dpi":null}"#).0,
            "a null dpi is not a number"
        );
        assert!(!check(r#"{"platform":"win32","arch":"x64","build":22000.5}"#).0);
        assert_eq!(check("{}").1.len(), 3);
    }
}
