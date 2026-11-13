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
    /#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color|color-mix)\((?:[^()]|\([^()]*\))*\)|\b[a-z]+\b/gi;
  const pixel = document.createElement('canvas');
  pixel.width = pixel.height = 1;
  const context = pixel.getContext('2d', { willReadFrequently: true });

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
      } else {
        // Another colour space, such as oklch() or color(): paint it and
        // read back the sRGB pixel.
        context.clearRect(0, 0, 1, 1);
        context.fillRect(0, 0, 1, 1);
        const [r, g, b, a] = context.getImageData(0, 0, 1, 1).data;
        color = { r: r / 255, g: g / 255, b: b / 255, a: a / 255 };
      }
    }
    if (S.parsed.size > 4096) S.parsed.clear();
    S.parsed.set(key, color);
    return color;
  };

  const linear = (v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
  // The contrast ratio between two colours, from 1 to 21.
  const contrast = (one, two) => {
    const luminance = ({ r, g, b }) =>
      0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
    const [high, low] = [luminance(one), luminance(two)].sort((x, y) => y - x);
    return (high + 0.05) / (low + 0.05);
  };

  // Lightness, chroma and hue in OKLab, where equal steps look equal.
  const oklch = ({ r, g, b }) => {
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
  // The theme colours the stylesheet gives these elements where they read.
  const own = [
    ['a', parse((T.variables || {}).accent || '')],
    ['code, kbd, samp', parse((T.variables || {}).code || '')],
    ['blockquote', parse((T.variables || {}).muted || '')],
  ];
  const OWN = own.map(([selector]) => selector).join(', ');
  // The default fills the stylesheet gives these, which the text in them
  // must read on.
  const fills = [
    ['pre', parse((T.variables || {}).block || '')],
    ['th', parse((T.variables || {})['table-head'] || '')],
    ['code, kbd, samp', parse((T.variables || {})['code-fill'] || '')],
  ];
  const FILLS = fills.map(([selector]) => selector).join(', ');
  // Each theme colour's hue, and where it sits between the theme's page (0)
  // and text (1).
  const hues = (T.hues || []).map((value) => {
    const color = parse(value);
    const { l, h } = oklch(color);
    const [page, text] = [oklch(theme.page).l, oklch(theme.text).l];
    return { color, h, at: Math.min(1, Math.max(0.05, (l - page) / (text - page))) };
  });
  // Where a neutral sits between the page (0) and its text (1), in the theme.
  const ramp = [
    [0, theme.page], [0.07, theme.tint], [0.2, theme.lineStrong], [0.4, theme.faint],
    [0.6, theme.muted], [0.9, theme.text2], [1, theme.text],
  ];
  // The document's own page and text lightness, measured before mapping.
  let page = { from: 1, span: -1 };

  // `color` in the theme. `fill` says whether it is a background (true), a
  // foreground such as text or a border (false), or unknown (null), as in a
  // custom property, where a colour close to the page is taken as a fill.
  const mapped = (color, fill) => {
    const { l, c, h } = oklch(color);
    const at = Math.min(1, (l - page.from) / page.span);
    if (c >= 0.045 && hues.length) {
      const turn = (d) => Math.abs(Math.atan2(Math.sin(d), Math.cos(d)));
      const hue = hues.reduce((best, next) => (turn(next.h - h) < turn(best.h - h) ? next : best));
      // The theme's colour for that hue, as far from the page as the authored
      // colour was, so a deep badge under light text stays deep and dark text
      // on a pale badge stays dark. A foreground reaches the full colour by
      // halfway to the text colour, where most text colours sit.
      const asFill = fill === null ? at < 0.35 : fill;
      const reach = asFill ? hue.at : Math.min(hue.at, 0.55);
      const amount = Math.min(1, Math.max(0.16, at / reach));
      return css(mix(theme.page, hue.color, amount), color.a);
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

  // Custom properties used as the channels of a colour, as in
  // `rgb(var(--bg))` or `hsl(var(--background))`, with that function. CSS
  // resolves them on each element, so they are mapped where they are defined.
  let channels = new Map();
  // Properties defined as another one, as in `--panel: var(--base)`: a
  // channel property's alias makes the property it names one too.
  let aliases = new Map();
  const CHANNELS = /\b(rgba?|hsla?)\(\s*var\(\s*(--[\w-]+)\s*[,)]/gi;
  const ALIASES = /(--[\w-]+)\s*:\s*var\(\s*(--[\w-]+)/g;
  // Note the channel properties and aliases `text` uses; whether any channel
  // property was new.
  const learn = (text) => {
    let fresh = false;
    const add = (name, fn) => {
      if (channels.has(name)) return;
      channels.set(name, fn);
      fresh = true;
      for (const target of aliases.get(name) || []) add(target, fn);
    };
    for (const [, name, target] of text.matchAll(ALIASES)) {
      if (!aliases.has(name)) aliases.set(name, new Set());
      aliases.get(name).add(target);
      if (channels.has(name)) add(target, channels.get(name));
    }
    for (const [, fn, name] of text.matchAll(CHANNELS)) {
      add(name, fn.toLowerCase().startsWith('hsl') ? 'hsl' : 'rgb');
    }
    return fresh;
  };

  // Channels in the theme, written as they were: `0 0 0`, `0, 0, 0`, or for
  // hsl `0 0% 0%`, with any alpha they carry. An alias keeps its reference
  // and has its fallback mapped.
  const rechannel = (value, fn) => {
    const alias = /^\s*var\(\s*(--[\w-]+)\s*,\s*([^]+)\)\s*$/.exec(value);
    if (alias) {
      const fallback = rechannel(alias[2], fn);
      return fallback ? `var(${alias[1]}, ${fallback})` : null;
    }
    let tail = '';
    const slash = value.indexOf('/');
    const commas = value.split(',');
    if (slash >= 0) {
      tail = ` / ${value.slice(slash + 1).trim()}`;
      value = value.slice(0, slash);
    } else if (commas.length === 4) {
      tail = `, ${commas[3].trim()}`;
      value = commas.slice(0, 3).join(',');
    }
    const color = parse(`${fn}(${value})`);
    if (!color || color.a <= 0) return null;
    const { r, g, b } = parse(mapped({ ...color, a: 1 }, null));
    let parts = [r, g, b].map((v) => String(Math.round(v * 255)));
    if (fn === 'hsl') {
      const max = Math.max(r, g, b);
      const min = Math.min(r, g, b);
      const l = (max + min) / 2;
      const d = max - min;
      const s = d ? d / (1 - Math.abs(2 * l - 1)) : 0;
      let h = 0;
      if (d) h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
      parts = [`${+((h * 60 + 360) % 360).toFixed(1)}`, `${+(s * 100).toFixed(1)}%`, `${+(l * 100).toFixed(1)}%`];
    }
    return parts.join(value.includes(',') ? ', ' : ' ') + tail;
  };

  // A colour function whose alpha is a var(), as in `rgb(1 2 3 / var(--a))`:
  // its colour is mapped and its alpha kept. A var() elsewhere in it is a
  // channel or colour property, mapped where it is defined.
  const ALPHA = /^([a-z]+)\((.*?)(?:\s*\/\s*|\s*,\s*)(var\(.*\))\)$/is;
  const alpha = (token, fill) => {
    const match = ALPHA.exec(token);
    if (!match || match[2].includes('var(')) return token;
    const color = parse(`${match[1]}(${match[2]})`);
    if (!color) return token;
    const { r, g, b } = parse(mapped({ ...color, a: 1 }, fill));
    const part = (v) => Math.round(v * 255);
    return `rgb(${part(r)} ${part(g)} ${part(b)} / ${match[3]})`;
  };

  // A colour function with a var() in it: an alpha var() keeps its alpha; a
  // channel var()'s fallback channels are mapped, as in
  // `rgb(var(--bg, 255 255 255))`; otherwise the literal colours among its
  // arguments are, as in `color-mix(in srgb, var(--accent) 10%, white)`.
  const within = (token, fill) => {
    const kept = alpha(token, fill);
    if (kept !== token) return kept;
    const call = /^([a-z-]+)\(([^]*)\)$/i.exec(token);
    if (!call) return token;
    const channel = /^\s*var\(\s*(--[\w-]+)\s*,\s*([^]+)\)\s*$/.exec(call[2]);
    if (/^(rgba?|hsla?)$/i.test(call[1]) && channel) {
      const fn = call[1].toLowerCase().startsWith('hsl') ? 'hsl' : 'rgb';
      const mapped = rechannel(channel[2], fn);
      return mapped ? `${call[1]}(var(${channel[1]}, ${mapped}))` : token;
    }
    return `${call[1]}(${recolor(call[2], fill)})`;
  };

  // `value` with each colour in it mapped, as a fill or a foreground.
  const recolor = (value, fill) => {
    if (value.includes('url(')) return value;
    return value.replace(TOKEN, (token, at) => {
      const before = value[at - 1] || '';
      const after = value[at + token.length] || '';
      const word = /^[a-z]+$/i.test(token);
      if (word && (/[-\w]/.test(before) || /[-\w(]/.test(after))) return token;
      if (token.includes('var(')) return within(token, fill);
      const color = parse(token);
      return color && color.a > 0 ? mapped(color, fill) : token;
    });
  };

  // Shorthands that carry a colour. Written with var(), their longhands read
  // as empty until the page is drawn, so the shorthand itself is mapped.
  const SHORTHAND =
    /^(?:background|border(?:-(?:top|right|bottom|left|block|inline)(?:-(?:start|end))?)?|outline|text-decoration|column-rule|text-emphasis)$/;

  const names = (style) => {
    const found = new Set(Array.from(style));
    for (const [, name] of style.cssText.matchAll(/(?:^|[;{\s])(--[\w-]+|[a-z-]+)\s*:/g)) {
      if (name.startsWith('--') || SHORTHAND.test(name)) found.add(name);
    }
    return found;
  };

  // Put back what the document wrote, unless it has since written anew.
  const restore = (style) => {
    for (const [name, value, priority, applied] of S.saved.get(style) || []) {
      if (style.getPropertyValue(name) === applied) style.setProperty(name, value, priority);
    }
    S.saved.delete(style);
  };

  // A rule that reaches only SVG content, which keeps its own colours like
  // the graphic's inline styles; a rule matching nothing yet is mapped.
  // State pseudo-classes are set aside, so `svg:hover` counts before a hover.
  const STATE = /:(?:hover|focus|focus-within|focus-visible|active|visited|checked|target)\b/g;
  const graphic = (rule) => {
    try {
      const found = document.querySelectorAll(rule.selectorText.replace(STATE, ''));
      return found.length > 0 && Array.from(found).every((element) => element.closest('svg'));
    } catch {
      return false;
    }
  };

  const remap = (style, rule) => {
    const changes = [];
    for (const name of names(style)) {
      const value = style.getPropertyValue(name);
      if (!value) continue;
      let next = null;
      if (name.startsWith('--') && !name.startsWith('--ai-')) {
        // A property that holds one colour, a gradient, or another property
        // with a fallback; a shadow or a list keeps its own colours.
        const color = parse(value);
        if (color) {
          if (color.a > 0) next = mapped(color, null);
        } else if (channels.has(name)) {
          next = rechannel(value, channels.get(name));
        } else if (/gradient\(|^\s*var\(|^\s*[a-z-]+\([^]*\)\s*$/i.test(value)) {
          next = recolor(value, null);
        }
      } else if (COLOR.test(name) || (SHORTHAND.test(name) && value.includes('var('))) {
        next = recolor(value, name.startsWith('background'));
      }
      if (next && next !== value) changes.push([name, value, style.getPropertyPriority(name), next]);
    }
    if (!changes.length) return;
    if (rule && graphic(rule)) {
      S.kept.push(rule);
      return;
    }
    for (const change of changes) {
      style.setProperty(change[0], change[3], change[2]);
      change[3] = style.getPropertyValue(change[0]);
    }
    S.saved.set(style, changes);
  };

  // Whether any rule depends on hover, focus or press, which change fills
  // without changing the document.
  const STATES = /:(?:hover|focus|focus-within|focus-visible|active)\b/;
  let interactive = false;
  // Whether the document styles its own selection.
  let selecting = false;

  // Every style declaration the document wrote: rules, nested rules, inline.
  const declarations = () => {
    const out = [];
    interactive = false;
    selecting = false;
    const walk = (rules) => {
      for (const rule of rules) {
        if (rule.type === CSSRule.KEYFRAMES_RULE || rule.type === CSSRule.FONT_FACE_RULE) continue;
        if (rule.style) out.push([rule.style, rule.selectorText ? rule : null]);
        if (STATES.test(rule.selectorText || '')) interactive = true;
        if ((rule.selectorText || '').includes('::selection')) selecting = true;
        if (rule.cssRules) walk(rule.cssRules);
        // An imported sheet the document may read, such as a data: URL.
        if (rule.styleSheet) {
          try {
            walk(rule.styleSheet.cssRules);
          } catch {
            // One it may not read keeps its colours.
          }
        }
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
      if (!element.closest('svg')) out.push([element.style, null]);
    }
    return out;
  };

  const measure = () => {
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

  // The fill an element is read against: its own, else the nearest behind
  // it; null when that is an image or gradient.
  // Translucent fills are laid over what is behind them.
  const behind = (element) => {
    const layers = [];
    let base = theme.page;
    for (let at = element; at; at = at.parentElement) {
      const style = getComputedStyle(at);
      if (style.backgroundImage !== 'none') return null;
      const fill = parse(style.backgroundColor);
      if (!fill || fill.a <= 0) continue;
      if (fill.a >= 0.999) {
        base = fill;
        break;
      }
      layers.push(fill);
    }
    return layers.reduceRight((under, layer) => mix(under, layer, layer.a), base);
  };

  // Mark `element` when the theme's `color` would not read against what is
  // behind it. The mark says which of the theme's text and page colours
  // reads better there, for an element the document gives no colour.
  const mark = (element, color) => {
    const fill = behind(element);
    if (fill && color && contrast(fill, color) >= 3) {
      element.removeAttribute('data-ai-own');
      return;
    }
    const ink = fill && contrast(fill, theme.text) >= contrast(fill, theme.page);
    element.setAttribute('data-ai-own', fill ? (ink ? 'ink' : 'page') : '');
  };

  // Whether every piece of text in `element` reads on `fill`, apart from
  // parts that have a fill of their own.
  const fits = (element, fill) => {
    const seen = new Set();
    for (const part of [element, ...element.querySelectorAll('*')]) {
      const text = Array.from(part.childNodes).some(
        (node) => node.nodeType === Node.TEXT_NODE && node.data.trim(),
      );
      if (!text) continue;
      const style = getComputedStyle(part);
      if (part !== element && (parse(style.backgroundColor)?.a || 0) > 0.5) continue;
      if (seen.has(style.color)) continue;
      seen.add(style.color);
      const ink = parse(style.color);
      if (!ink || contrast(ink, fill) < 3) return false;
    }
    return true;
  };

  // The stylesheet gives links, inline code and quotes the theme's link, code
  // and muted colours, except where that would be hard to read against what
  // is behind them; those keep the text colour mapped for their fill.
  const readable = (root) => {
    if (!root) return;
    // A code block or header takes the default fill only where all its text
    // reads on it; otherwise it keeps the fill behind it, such as its row's.
    const blocks = root.matches?.(FILLS) ? [root] : [];
    for (const element of [...blocks, ...root.querySelectorAll(FILLS)]) {
      if (!element.matches('pre, th') && element.closest('pre')) continue;
      const fill = fills.find(([selector]) => element.matches(selector))[1];
      element.toggleAttribute('data-ai-own-fill', !fill || !fits(element, fill));
    }
    const found = root.matches?.(OWN) ? [root] : [];
    for (const element of [...found, ...root.querySelectorAll(OWN)]) {
      // Code in a code block takes the block's colour; a link there is still
      // given the link colour, so it is checked.
      if (element.closest('svg') || (!element.matches('a') && element.closest('pre'))) continue;
      // Inside a link the link colour wins, so that is the colour to check.
      const link = element.parentElement?.closest('a');
      const color = (link ? own[0] : own.find(([selector]) => element.matches(selector)))[1];
      mark(element, color);
      // The link colour reaches inside a link too: a part with its own fill
      // keeps its own colour where the link colour would not read on it.
      if (!element.matches('a')) continue;
      for (const part of element.querySelectorAll('*')) {
        const style = getComputedStyle(part);
        const filled = style.backgroundImage !== 'none' || (parse(style.backgroundColor)?.a || 0) > 0;
        if (filled) mark(part, color);
        else part.removeAttribute('data-ai-own');
      }
    }
  };

  // Held while colours are restored, measured and mapped, so the document's
  // own transitions neither show the old colours nor are read mid-way.
  const HOLD = '*,*::before,*::after{transition:none!important;}';
  // Selected text keeps its own colour over a translucent accent, as in
  // native readers; a document that styles its selection keeps that, mapped.
  const SELECTION = '::selection{background:var(--ai-selection)!important;}';
  const settle = () => getComputedStyle(document.documentElement).color;

  // SVG keeps its authored colours, including those it takes from custom
  // properties set outside it: each outermost graphic whose values the
  // mapping changed holds the authored ones through a host rule with no
  // specificity, so the document's own rules for the graphic, such as
  // svg:hover, and its inline style still win.
  const pin = (all) => {
    for (const svg of document.querySelectorAll('[data-ai-pin]')) svg.removeAttribute('data-ai-pin');
    S.pins = 0;
    const props = new Set();
    for (const [style] of all) {
      for (const name of names(style)) {
        if (name.startsWith('--') && !name.startsWith('--ai-')) props.add(name);
      }
    }
    if (!props.size) return () => '';
    const graphics = Array.from(document.querySelectorAll('svg')).filter(
      (svg) => !svg.parentElement?.closest('svg'),
    );
    const authored = graphics.map((svg) => {
      const style = getComputedStyle(svg);
      return [svg, [...props].map((name) => [name, style.getPropertyValue(name)])];
    });
    return () => {
      let css = '';
      authored.forEach(([svg, values], index) => {
        // Only the properties whose value here the mapping changed, aliases
        // of mapped properties included: a layout value such as a width keeps
        // following its media queries.
        const now = getComputedStyle(svg);
        const held = values
          .filter(([name, value]) => value && now.getPropertyValue(name) !== value)
          .map(([name, value]) => `${name}:${value.trim()};`)
          .join('');
        if (!held) return;
        svg.setAttribute('data-ai-pin', index);
        css += `:where(svg[data-ai-pin="${index}"]){${held}}`;
        S.pins += 1;
      });
      return css;
    };
  };

  const apply = () => {
    if (!document.documentElement) return;
    S.observer?.disconnect();
    S.busy = true;
    try {
      const style = host();
      style.textContent = HOLD;
      const all = declarations();
      all.forEach(([style]) => restore(style));
      const pinned = pin(all);
      S.kept = [];
      channels = new Map();
      aliases = new Map();
      for (const [style] of all) learn(style.cssText);
      measure();
      all.forEach(([style, rule]) => remap(style, rule));
      const held = pinned();
      const variables = Object.entries(T.variables || {})
        .map(([name, value]) => `--ai-${name}:${value}!important;`)
        .join('');
      const css = `:root{color-scheme:${T.scheme}!important;${variables}}${T.css}${
        selecting ? '' : SELECTION
      }${held}`;
      style.textContent = HOLD + css;
      readable(document);
      // Releasing the hold after the new colours are in place animates nothing.
      settle();
      style.textContent = css;
      S.interactive = interactive;
    } finally {
      S.busy = false;
    }
    S.observer?.takeRecords();
    watch();
  };

  // One element's inline style, after the document changed it.
  const touch = (element) => {
    if (!(element instanceof Element) || element.closest('svg')) return;
    // A new channel property may be defined in rules mapped without it.
    if (learn(element.style.cssText)) S.schedule();
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

  // Check every link and code element again, once, shortly after: a change
  // can reach elements outside its own subtree through selectors such as
  // `.selected + a` or `:has()`.
  S.readable = readable;
  S.recheck = () => {
    const state = window[KEY];
    if (state.rechecking) return;
    state.rechecking = true;
    setTimeout(() => {
      state.rechecking = false;
      state.readable(document);
    }, 30);
  };

  // Hover, focus and press change fills without a DOM record: check again
  // when the document has rules for those states, and whenever a transition
  // or animation ends.
  if (!S.listening) {
    S.listening = true;
    const recheck = (always) => () => {
      const state = window[KEY];
      if (!always && !state.interactive) return;
      // A state rule around a graphic can change the values it holds.
      if (!always && state.pins) state.schedule();
      else state.recheck();
    };
    for (const name of ['pointerover', 'pointerout', 'pointerdown', 'pointerup', 'focusin', 'focusout']) {
      document.addEventListener(name, recheck(false), true);
    }
    // A fill that moves to its new colour is read again where it settles.
    for (const name of ['transitionend', 'animationend']) {
      document.addEventListener(name, recheck(true), true);
    }
    // A style sheet that finishes loading later, such as a replaced href,
    // brings rules that were not there to map.
    document.addEventListener(
      'load',
      (event) => {
        const node = event.target;
        if (node && node.id !== ID && (node.nodeName === 'LINK' || node.nodeName === 'STYLE')) {
          window[KEY].schedule();
        }
      },
      true,
    );
    // The document's own light or dark rules take over when the system
    // appearance changes, so its page and text are measured again.
    window.matchMedia?.('(prefers-color-scheme: dark)').addEventListener?.('change', () => {
      window[KEY].schedule();
    });
    // A media query can change fills, and the values a graphic holds.
    window.addEventListener('resize', () => {
      const state = window[KEY];
      state.recheck();
      // Pins may be needed only at the new size, so any graphic counts.
      if (!document.querySelector('svg')) return;
      clearTimeout(state.resizing);
      state.resizing = setTimeout(() => state.apply(), 150);
    });
  }

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
      attributes: true,
    });
  };

  // A style element or style sheet link of the document's own.
  const sheet = (node) =>
    node && node.id !== ID && (node.nodeName === 'STYLE' || node.nodeName === 'LINK');

  S.changed = (records) => {
    // A node added or removed with style sheets or a graphic inside it.
    const holds = (node) =>
      sheet(node) ||
      (node instanceof Element && (node.matches('svg') || Boolean(node.querySelector('style, link, svg'))));
    // The root and body may carry the document's own theme: a class, a data
    // attribute or custom properties that expressions were expanded with.
    const top = (node) => node === document.documentElement || node === document.body;
    let whole = false;
    const elements = new Set();
    let any = false;
    for (const record of records) {
      if (record.type === 'attributes') {
        if (record.attributeName.startsWith('data-ai-')) continue;
        any = true;
        // A sheet's href, media or disabled state changes which rules apply,
        // and a change around a graphic can change the values it holds.
        if (top(record.target) || sheet(record.target)) whole = true;
        else if (record.target.querySelector?.('svg')) whole = true;
        // The graphic's own class or attributes can match rules that set the
        // properties it holds; its own style changes need no new pins.
        else if (record.target.matches?.('svg') && record.attributeName !== 'style') whole = true;
        // Only a style attribute holds colours; any other attribute can still
        // change a fill through a selector, such as [aria-selected], which the
        // check below covers.
        else if (record.attributeName === 'style') elements.add(record.target);
      } else if (record.type === 'characterData') {
        any = true;
        whole ||= sheet(record.target.parentNode);
      } else {
        any = true;
        whole ||= sheet(record.target);
        for (const node of [...record.addedNodes, ...record.removedNodes]) {
          whole ||= holds(node);
        }
        for (const node of record.addedNodes) {
          if (!(node instanceof Element)) continue;
          if (node.hasAttribute('style')) elements.add(node);
          node.querySelectorAll('[style]').forEach((element) => elements.add(element));
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
    } finally {
      S.busy = false;
    }
    S.observer.takeRecords();
    watch();
    // A rule kept for SVG alone that now reaches other elements, as when a
    // paragraph takes a class only SVG text had, is mapped with the rest.
    if (any && (S.kept || []).some((rule) => !graphic(rule))) {
      S.schedule();
      return;
    }
    // A changed fill changes which links and code read against it.
    if (any) S.recheck();
  };
  S.apply = apply;

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', apply, { once: true });
  } else {
    apply();
  }
  if (document.readyState !== 'complete') window.addEventListener('load', apply, { once: true });
})();
