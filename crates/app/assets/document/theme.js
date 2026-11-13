// Draws an HTML document in the app's theme. The host evaluates this file at
// document start and again whenever the theme changes, with the theme's
// colours in place of the palette placeholder below.
//
// A document keeps its own layout and the meaning of its colours. Each colour
// in its style rules and inline styles is replaced by the theme colour playing
// the same part: a neutral by where it sits between the document's own
// background and text colour, a chromatic colour by its hue. The authored
// values are kept, so a theme change maps from them again without a reload.
(() => {
  const T = /*PALETTE*/ {};
  const ID = 'agent-interface-theme';
  const KEY = Symbol.for('agent-interface.theme');
  const S = window[KEY] || (window[KEY] = { saved: new WeakMap(), parsed: new Map() });
  const KEEP = new Set([
    'transparent', 'currentcolor', 'inherit', 'initial', 'unset', 'revert',
    'revert-layer', 'none', 'auto',
  ]);
  const COLOR = /(?:^|-)color$|^background-image$/;
  const TOKEN =
    /#[0-9a-f]{3,8}\b|\b(?:rgb|hsl)a?\((?:[^()]|\([^()]*\))*\)|\b[a-z]+\b/gi;
  const context = document.createElement('canvas').getContext('2d');

  // A CSS colour as { r, g, b, a } with channels in 0..1, or null.
  const parse = (text) => {
    const key = text.trim().toLowerCase();
    if (!key || KEEP.has(key) || !context) return null;
    if (S.parsed.has(key)) return S.parsed.get(key);
    const read = (first) => {
      context.fillStyle = first;
      context.fillStyle = key;
      return context.fillStyle;
    };
    const value = read('#000000');
    let color = null;
    if (value === read('#ffffff')) {
      const hex = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/.exec(value);
      const rgba = /^rgba\((\d+), (\d+), (\d+), ([\d.]+)\)$/.exec(value);
      if (hex) {
        const [r, g, b] = hex.slice(1).map((part) => parseInt(part, 16) / 255);
        color = { r, g, b, a: 1 };
      } else if (rgba) {
        color = { r: rgba[1] / 255, g: rgba[2] / 255, b: rgba[3] / 255, a: +rgba[4] };
      }
    }
    if (S.parsed.size > 4096) S.parsed.clear();
    S.parsed.set(key, color);
    return color;
  };

  // Lightness, chroma and hue in OKLab, where equal steps look equal.
  const oklch = ({ r, g, b }) => {
    const linear = (v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
    const [lr, lg, lb] = [linear(r), linear(g), linear(b)];
    const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
    const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
    const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
    const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
    const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
    return {
      l: 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
      c: Math.hypot(A, B),
      h: Math.atan2(B, A),
    };
  };
  const mix = (from, to, amount) => ({
    r: from.r + (to.r - from.r) * amount,
    g: from.g + (to.g - from.g) * amount,
    b: from.b + (to.b - from.b) * amount,
  });
  const css = ({ r, g, b }, alpha) => {
    const part = (v) => Math.round(Math.min(1, Math.max(0, v)) * 255);
    return alpha >= 1
      ? `rgb(${part(r)}, ${part(g)}, ${part(b)})`
      : `rgba(${part(r)}, ${part(g)}, ${part(b)}, ${+alpha.toFixed(3)})`;
  };

  const theme = Object.fromEntries(
    Object.entries(T.ramp || {}).map(([name, value]) => [name, parse(value)]),
  );
  const hues = (T.hues || []).map((value) => {
    const color = parse(value);
    return { color, h: oklch(color).h };
  });
  // Where a neutral sits between the page (0) and its text (1), in the theme.
  const ramp = [
    [0, theme.page], [0.07, theme.tint], [0.2, theme.lineStrong], [0.4, theme.faint],
    [0.6, theme.muted], [0.9, theme.text2], [1, theme.text],
  ];
  // The document's own page and text lightness, measured before mapping.
  let page = { from: 1, span: -1 };

  const mapped = (color) => {
    const { l, c, h } = oklch(color);
    const at = Math.min(1, (l - page.from) / page.span);
    if (c >= 0.045 && hues.length) {
      const turn = (d) => Math.abs(Math.atan2(Math.sin(d), Math.cos(d)));
      const hue = hues.reduce((best, next) => (turn(next.h - h) < turn(best.h - h) ? next : best));
      // Near the page it is a fill behind text; otherwise the colour itself.
      return css(at < 0.3 ? mix(theme.page, hue.color, 0.16) : hue.color, color.a);
    }
    // Lighter than a light page, or darker than a dark one: a raised card.
    if (at < -0.005) return css(theme.raised, color.a);
    let below = ramp[0];
    for (const stop of ramp) {
      if (at <= stop[0]) {
        const span = stop[0] - below[0];
        return css(span ? mix(below[1], stop[1], (at - below[0]) / span) : stop[1], color.a);
      }
      below = stop;
    }
    return css(theme.text, color.a);
  };

  // What the document's custom properties hold on its root, before mapping.
  let roots = new Map();

  // `value` with each colour in it mapped. A custom property inside a larger
  // expression, as in `rgb(1 2 3 / var(--alpha))`, reads as its root value; a
  // value that is one `var()` alone follows that property's own mapping.
  const recolor = (authored) => {
    if (authored.includes('url(')) return authored;
    let value = authored;
    if (value.includes('var(') && !/^\s*var\([^()]*\)\s*$/.test(value)) {
      value = value.replace(
        /var\(\s*(--[\w-]+)\s*(?:,([^()]*))?\)/g,
        (whole, name, fallback) => roots.get(name) || (fallback && fallback.trim()) || whole,
      );
    }
    return value.replace(TOKEN, (token, at) => {
      const before = value[at - 1] || '';
      const after = value[at + token.length] || '';
      const word = /^[a-z]+$/i.test(token);
      if (word && (/[-\w]/.test(before) || /[-\w(]/.test(after))) return token;
      const color = parse(token);
      return color && color.a > 0 ? mapped(color) : token;
    });
  };

  const names = (style) => {
    const found = new Set(Array.from(style));
    for (const [, name] of style.cssText.matchAll(/(?:^|[;{\s])(--[\w-]+)\s*:/g)) found.add(name);
    return found;
  };

  // Put back what the document wrote, unless it has since written anew.
  const restore = (style) => {
    for (const [name, value, priority, applied] of S.saved.get(style) || []) {
      if (style.getPropertyValue(name) === applied) style.setProperty(name, value, priority);
    }
    S.saved.delete(style);
  };

  const remap = (style) => {
    const changes = [];
    for (const name of names(style)) {
      const value = style.getPropertyValue(name);
      if (!value) continue;
      let next = null;
      if (name.startsWith('--')) {
        const color = name.startsWith('--ai-') ? null : parse(value);
        if (color && color.a > 0) next = mapped(color);
      } else if (COLOR.test(name)) {
        next = recolor(value);
      }
      if (next && next !== value) changes.push([name, value, style.getPropertyPriority(name), next]);
    }
    if (!changes.length) return;
    for (const change of changes) {
      style.setProperty(change[0], change[3], change[2]);
      change[3] = style.getPropertyValue(change[0]);
    }
    S.saved.set(style, changes);
  };

  // Every style declaration the document wrote: rules, nested rules, inline.
  const declarations = () => {
    const out = [];
    const walk = (rules) => {
      for (const rule of rules) {
        if (rule.type === CSSRule.KEYFRAMES_RULE || rule.type === CSSRule.FONT_FACE_RULE) continue;
        if (rule.style) out.push(rule.style);
        if (rule.cssRules) walk(rule.cssRules);
      }
    };
    for (const sheet of [...document.styleSheets, ...(document.adoptedStyleSheets || [])]) {
      if (sheet.ownerNode && sheet.ownerNode.id === ID) continue;
      try {
        walk(sheet.cssRules);
      } catch {
        // A sheet the document may not read keeps its colours.
      }
    }
    for (const element of document.querySelectorAll('[style]')) {
      if (!element.closest('svg')) out.push(element.style);
    }
    return out;
  };

  const measure = (all) => {
    const root = getComputedStyle(document.documentElement);
    roots = new Map();
    for (const style of all) {
      for (const name of names(style)) {
        if (!name.startsWith('--') || roots.has(name)) continue;
        const value = root.getPropertyValue(name).trim();
        if (value) roots.set(name, value);
      }
    }
    const solid = (element, property) => {
      const color = element && parse(getComputedStyle(element)[property]);
      return color && color.a > 0.5 ? oklch(color).l : null;
    };
    const text = solid(document.body || document.documentElement, 'color') ?? 0;
    const from =
      solid(document.body, 'backgroundColor') ??
      solid(document.documentElement, 'backgroundColor') ??
      (text > 0.6 ? 0 : 1);
    // Text too close to its page says nothing about which end is which.
    page = Math.abs(text - from) < 0.2
      ? { from: text > 0.5 ? 0 : 1, span: text > 0.5 ? 1 : -1 }
      : { from, span: text - from };
  };

  const host = () => {
    let style = document.getElementById(ID);
    if (!style) {
      style = document.createElement('style');
      style.id = ID;
    }
    const parent = document.head || document.documentElement;
    if (parent.lastElementChild !== style) parent.append(style);
    return style;
  };

  // A link with its own fill keeps the text colour mapped for that fill;
  // the stylesheet gives every other link the theme's link colour.
  const links = (root) => {
    const found = root.matches?.('a') ? [root] : [];
    for (const link of [...found, ...root.querySelectorAll('a')]) {
      const style = getComputedStyle(link);
      const fill = parse(style.backgroundColor);
      link.toggleAttribute(
        'data-ai-filled',
        Boolean(fill && fill.a > 0.5) || style.backgroundImage !== 'none',
      );
    }
  };

  const apply = () => {
    if (!document.documentElement) return;
    S.observer?.disconnect();
    S.busy = true;
    try {
      const style = host();
      style.textContent = '';
      const all = declarations();
      all.forEach(restore);
      measure(all);
      all.forEach(remap);
      const variables = Object.entries(T.variables || {})
        .map(([name, value]) => `--ai-${name}:${value}!important;`)
        .join('');
      style.textContent = `:root{color-scheme:${T.scheme}!important;${variables}}${T.css}`;
      links(document);
    } finally {
      S.busy = false;
    }
    S.observer?.takeRecords();
    watch();
  };

  // One element's inline style, after the document changed it.
  const touch = (element) => {
    if (!(element instanceof Element) || element.closest('svg')) return;
    restore(element.style);
    remap(element.style);
  };

  // Map the whole document again, once, shortly after a change to its rules.
  S.schedule = () => {
    if (S.queued) return;
    S.queued = true;
    // A timer, not a frame: a hidden page gets no frames.
    setTimeout(() => {
      S.queued = false;
      window[KEY].apply();
    }, 16);
  };

  // Rules changed through the style sheet objects leave no DOM record.
  if (!S.patched) {
    S.patched = true;
    const notice = (prototype, name, rules) => {
      const original = prototype[name];
      if (typeof original !== 'function') return;
      prototype[name] = function (...values) {
        const result = original.apply(this, values);
        const state = window[KEY];
        if (!state.busy && (!rules || this.parentRule)) {
          if (result instanceof Promise) result.then(state.schedule, () => {});
          else state.schedule();
        }
        return result;
      };
    };
    for (const name of ['insertRule', 'deleteRule', 'replace', 'replaceSync']) {
      notice(CSSStyleSheet.prototype, name, false);
    }
    for (const name of ['setProperty', 'removeProperty']) {
      notice(CSSStyleDeclaration.prototype, name, true);
    }
  }

  const watch = () => {
    S.observer = S.observer || new MutationObserver((records) => window[KEY].changed(records));
    S.observer.observe(document, {
      subtree: true, childList: true, characterData: true,
      attributes: true, attributeFilter: ['style'],
    });
  };

  S.changed = (records) => {
    const sheet = (node) =>
      node && node.id !== ID && (node.nodeName === 'STYLE' || node.nodeName === 'LINK');
    let whole = false;
    const elements = new Set();
    const added = [];
    for (const record of records) {
      if (record.type === 'attributes') {
        elements.add(record.target);
      } else if (record.type === 'characterData') {
        whole ||= sheet(record.target.parentNode);
      } else {
        whole ||= sheet(record.target);
        for (const node of [...record.addedNodes, ...record.removedNodes]) {
          whole ||= sheet(node);
        }
        for (const node of record.addedNodes) {
          if (!(node instanceof Element)) continue;
          if (node.hasAttribute('style')) elements.add(node);
          node.querySelectorAll('[style]').forEach((element) => elements.add(element));
          added.push(node);
        }
      }
    }
    if (whole) {
      S.schedule();
      return;
    }
    S.observer.disconnect();
    S.busy = true;
    try {
      elements.forEach(touch);
      added.forEach(links);
    } finally {
      S.busy = false;
    }
    S.observer.takeRecords();
    watch();
  };
  S.apply = apply;

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', apply, { once: true });
  } else {
    apply();
  }
  if (document.readyState !== 'complete') window.addEventListener('load', apply, { once: true });
})();
