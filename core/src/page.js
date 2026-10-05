// What Syn runs inside a web page to see it and to find the thing to press.
//
// `cdp.rs` wraps this in a function, sets S.base to the handle's unit, and
// appends `return JSON.stringify(S.something(...))`. Nothing here is left in
// the page afterwards: no globals, no listeners, no marks on elements.
//
// Why it exists at all. The browser hand used to take a CSS selector from the
// model and run `querySelector` on it, which assumes a model can guess the
// selector of a page it has never seen. This is the other half: a map of
// what can be pressed or filled, each with a selector that finds exactly
// that one thing; selectors by visible text or label for what the map does
// not name; a way into frames and shadow roots, which `querySelector` cannot
// cross; and a check that the thing is really there to press (visible, not
// covered) before a real mouse press is sent to it.
//
// Every function returns a plain object; `cdp.rs` turns it into words. A
// refusal is {ok:false, why, say}: `why` is a short code for the code that
// decides, `say` is the sentence a model reads.

var S = { base: document };
var DEEP = ' >>> ';
var MAX_TEXT = 4000;

function norm(s) { return String(s == null ? '' : s).replace(/\s+/g, ' ').trim(); }
function cut(s, n) { s = norm(s); return s.length > n ? s.slice(0, n - 1) + '…' : s; }
function esc(s) { return (window.CSS && CSS.escape) ? CSS.escape(String(s)) : String(s).replace(/[^\w-]/g, '\\$&'); }
function fail(why, say) { return { ok: false, why: why, say: say }; }
function slice(l) { return Array.prototype.slice.call(l); }

function vis(e) {
  if (!e || e.nodeType !== 1) return false;
  if (e.checkVisibility && !e.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) return false;
  var r = e.getBoundingClientRect();
  return r.width > 0 && r.height > 0;
}

// ------------------------------------------------------------------ names

function labelOf(e) {
  var t = e.getAttribute && e.getAttribute('aria-label');
  if (norm(t)) return norm(t);
  var by = e.getAttribute && e.getAttribute('aria-labelledby');
  if (by) {
    var d = e.getRootNode();
    t = by.split(/\s+/).map(function (i) { var x = d.getElementById ? d.getElementById(i) : null; return x ? x.textContent : ''; }).join(' ');
    if (norm(t)) return norm(t);
  }
  if (e.labels && e.labels.length) {
    t = slice(e.labels).map(function (l) { return l.textContent; }).join(' ');
    if (norm(t)) return norm(t);
  }
  return norm(e.placeholder || e.title || e.alt || '');
}

function nameOf(e) {
  var tag = e.tagName;
  if (tag === 'INPUT' && /^(submit|button|reset|image)$/i.test(e.type)) return norm(e.value) || labelOf(e);
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return labelOf(e) || norm(e.name || e.id);
  var img = e.querySelector ? e.querySelector('img[alt]') : null;
  // The words of something hidden are not its name: an element the page
  // hides is where text meant for an agent and not for a person goes, and a
  // description of it ("div#secret ...") must not carry that text out.
  var shown = vis(e) ? (e.innerText !== undefined ? e.innerText : e.textContent) : '';
  return norm(e.getAttribute('aria-label') || shown || e.title || e.alt || (img ? img.alt : ''));
}

function editable(e) {
  var c = e.getAttribute && e.getAttribute('contenteditable');
  return c === '' || c === 'true' || c === 'plaintext-only';
}

function roleOf(e) {
  var r = e.getAttribute('role');
  var t = e.tagName.toLowerCase();
  if (t === 'a') return e.hasAttribute('href') ? 'link' : (r || '');
  if (t === 'button' || t === 'summary') return 'button';
  if (t === 'select') return 'select';
  if (t === 'textarea') return 'textbox';
  if (t === 'input') {
    var ty = (e.type || 'text').toLowerCase();
    if (ty === 'hidden') return '';
    if (/^(submit|button|reset|image)$/.test(ty)) return 'button';
    if (ty === 'checkbox' || ty === 'radio') return ty;
    return ty === 'text' ? 'textbox' : ty;
  }
  if (editable(e)) return 'textbox';
  if (r) return r;
  if (e.hasAttribute('onclick')) return 'clickable';
  return '';
}

function describe(e) {
  if (!e || !e.tagName) return 'nothing';
  var t = e.tagName.toLowerCase();
  var id = e.id ? '#' + e.id : '';
  var nm = (t !== 'html' && t !== 'body') ? cut(nameOf(e), 40) : '';
  return t + id + (nm ? ' "' + nm + '"' : '');
}

// Why a field must not be filled in by Syn: the person types these.
function secret(e) {
  var t = (e.type || '').toLowerCase();
  var ac = String((e.getAttribute && e.getAttribute('autocomplete')) || '').toLowerCase();
  if (t === 'password') return 'a password field';
  if (/(^|\s)(current-password|new-password|one-time-code|cc-number|cc-csc|cc-exp|cc-exp-month|cc-exp-year|cc-name|cc-type)(\s|$)/.test(ac)) return 'a password, code or card field';
  var w = norm([e.name, e.id, e.getAttribute && e.getAttribute('aria-label'), e.placeholder].join(' ')).toLowerCase();
  if (/(^|[^a-z])(password|passcode|passwd|pwd|cvv|cvc|card ?number|security code|ssn)([^a-z]|$)/.test(w)) return 'a password or card field';
  return '';
}

// -------------------------------------------------------------- selectors

function countIn(root, css) { try { return root.querySelectorAll(css).length; } catch (x) { return 0; } }

// A selector that finds this element and nothing else, within its own
// document or shadow root. Short and readable first: an id, a name, a test
// id, a label, a link address; a path of positions only when nothing else
// is unique.
function local(e) {
  var root = e.getRootNode(), t = e.tagName.toLowerCase(), c, i;
  if (e.id) { c = '#' + esc(e.id); if (countIn(root, c) === 1) return c; }
  var attrs = ['data-testid', 'data-test', 'data-qa', 'name', 'aria-label', 'placeholder', 'title', 'alt'];
  for (i = 0; i < attrs.length; i++) {
    var v = e.getAttribute(attrs[i]);
    if (v && v.length < 80) {
      c = t + '[' + attrs[i] + '=' + JSON.stringify(v) + ']';
      if (countIn(root, c) === 1) return c;
    }
  }
  if (t === 'a' && e.getAttribute('href')) {
    c = 'a[href=' + JSON.stringify(e.getAttribute('href')) + ']';
    if (countIn(root, c) === 1) return c;
  }
  var path = [], n = e, top = root.documentElement || null;
  while (n && n.nodeType === 1) {
    var idx = 1, s = n;
    while ((s = s.previousElementSibling)) { if (s.tagName === n.tagName) idx++; }
    path.unshift(n.tagName.toLowerCase() + ':nth-of-type(' + idx + ')');
    if (n.id && n !== e && countIn(root, '#' + esc(n.id)) === 1) { path.shift(); path.unshift('#' + esc(n.id)); break; }
    if (n === top) break;
    n = n.parentNode && n.parentNode.nodeType === 1 ? n.parentNode : null;
  }
  c = path.join(' > ');
  return c;
}

function sel(e) {
  var root = e.getRootNode();
  if (root && root.nodeType === 11 && root.host) return sel(root.host) + DEEP + local(e);
  var w = e.ownerDocument && e.ownerDocument.defaultView;
  if (w && w.frameElement) return sel(w.frameElement) + DEEP + local(e);
  return local(e);
}

var INTERACTIVE_TAG = /^(A|BUTTON|INPUT|SELECT|TEXTAREA|SUMMARY|LABEL)$/;
function interactiveUp(e, text) {
  // A match inside a button or link is that button or link.
  var p = e;
  while (p && p.parentElement) {
    var up = p.parentElement;
    if ((INTERACTIVE_TAG.test(up.tagName) || up.getAttribute('role') === 'button' || up.hasAttribute('onclick')) && norm(up.innerText || up.textContent).toLowerCase() === text) return up;
    if (norm(up.innerText || up.textContent).toLowerCase() !== text) break;
    p = up;
  }
  return e;
}

function byText(root, want) {
  var exact = false, m = /^"([\s\S]*)"$/.exec(want) || /^'([\s\S]*)'$/.exec(want);
  if (m) { want = m[1]; exact = true; }
  var w = norm(want).toLowerCase();
  if (!w) return [];
  var all = slice(root.querySelectorAll('*')), eq = [], has = [];
  for (var i = 0; i < all.length; i++) {
    var e = all[i];
    if (/^(SCRIPT|STYLE|NOSCRIPT|HEAD|META|LINK|TITLE|TEMPLATE|OPTION)$/.test(e.tagName)) continue;
    var tc = e.textContent || '';
    if (tc.length > w.length + 80 || tc.toLowerCase().indexOf(w) < 0) continue;
    if (!vis(e)) continue;
    var t = norm(e.innerText != null ? e.innerText : tc).toLowerCase();
    if (t === w) eq.push(e);
    else if (t.indexOf(w) >= 0) has.push(e);
  }
  var hits = (eq.length || exact) ? eq : has;
  // Keep the deepest: a <p> inside a <div> that says the same thing is one
  // match, and the thing to press is the inner one or its button.
  hits = hits.filter(function (e) { return !hits.some(function (o) { return o !== e && e.contains(o); }); });
  var out = [];
  hits.forEach(function (e) {
    var u = eq.length ? interactiveUp(e, w) : e;
    if (out.indexOf(u) < 0) out.push(u);
  });
  return out;
}

function byLabel(root, want) {
  var w = norm(want).toLowerCase();
  var all = slice(root.querySelectorAll('input,textarea,select,[contenteditable],[role=textbox],[role=combobox],button'));
  var eq = all.filter(function (e) { return nameOf(e).toLowerCase() === w; });
  if (eq.length) return eq;
  return all.filter(function (e) { return w && nameOf(e).toLowerCase().indexOf(w) >= 0; });
}

function candidates(root, part) {
  if (/^text=/i.test(part)) return byText(root, part.slice(5));
  if (/^label=/i.test(part)) return byLabel(root, part.slice(6));
  try { return slice(root.querySelectorAll(part)); }
  catch (x) { throw new Error('not a valid selector: ' + part + ' (CSS, or text=Words, or label=Words)'); }
}

function firstVisible(list) {
  for (var i = 0; i < list.length; i++) { if (vis(list[i])) return list[i]; }
  return list[0];
}

// The element a selector names. `strict` is for presses and typing: a
// selector by words that matches several things is refused with them, never
// guessed at; a CSS selector that matches several takes the first one that
// can be seen, and says how many there were.
function resolve(selector, strict) {
  var s = String(selector == null ? '' : selector).trim();
  if (s === '' || s === ':self' || s === ':doc') return { el: S.base === document ? document.documentElement : S.base, n: 1, all: [] };
  var parts = s.split(DEEP), root = S.base, el = null, all = [], i;
  for (i = 0; i < parts.length; i++) {
    var part = parts[i].trim();
    all = candidates(root, part);
    if (!all.length) throw new Error('no element matches ' + part + (i ? ' inside ' + describe(el) : ''));
    if (strict && all.length > 1 && /^(text|label)=/i.test(part)) {
      var shown = all.filter(vis);
      if (shown.length > 1) {
        throw new Error(shown.length + ' elements match ' + part + '. Use one of these selectors: ' +
          shown.slice(0, 6).map(function (e) { return sel(e) + ' (' + describe(e) + ')'; }).join('; ') + (shown.length > 6 ? '; and ' + (shown.length - 6) + ' more' : ''));
      }
      if (shown.length === 1) all = shown;
    }
    el = firstVisible(all);
    if (i < parts.length - 1) {
      if (el.tagName === 'IFRAME' || el.tagName === 'FRAME') {
        var d = null;
        try { d = el.contentDocument; } catch (x) { d = null; }
        if (!d) throw new Error('that frame is from another site, and the page does not let a script reach inside it');
        root = d;
      } else if (el.shadowRoot) {
        root = el.shadowRoot;
      } else {
        throw new Error('cannot look inside ' + describe(el) + ': it is not a frame and has no shadow root');
      }
    }
  }
  return { el: el, n: all.length, all: all };
}

function many(r) { return r.n > 1 ? ' (the first of ' + r.n + ' that match)' : ''; }

// ------------------------------------------------------------------ views

S.page = function () {
  var nav = (performance.getEntriesByType && performance.getEntriesByType('navigation')[0]) || {};
  var h = Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0);
  return {
    title: document.title, url: location.href, ready: document.readyState,
    status: nav.responseStatus || 0, y: Math.round(window.scrollY), h: h, vh: window.innerHeight
  };
};

S.read = function (selector, off) {
  var r = resolve(selector, false), e = r.el, t;
  if (/^(INPUT|TEXTAREA|SELECT)$/.test(e.tagName)) {
    if (secret(e)) return { ok: true, text: '(the value of ' + secret(e) + ' is never read)', total: 0, off: 0, n: r.n };
    t = String(e.value == null ? '' : e.value);
  } else if (e !== document.documentElement && e !== S.base && !vis(e)) {
    // What the person cannot see is not what the page says. `textContent`
    // would hand over a display:none block, which is exactly where text
    // meant for an agent and not for a person is put.
    return { ok: true, text: '', total: 0, off: 0, n: r.n, hidden: describe(e) };
  } else {
    // innerText is what is drawn; textContent is only for elements that
    // have none (SVG, MathML), never a fallback for an empty one.
    t = e.innerText !== undefined ? e.innerText : (e.textContent || '');
  }
  off = off | 0;
  return { ok: true, text: t.slice(off, off + MAX_TEXT), total: t.length, off: off, n: r.n };
};

S.map = function (budget) {
  budget = budget || 3600;
  var heads = [], fields = [], buttons = [], links = [], frames = 0, seen = 0, CAP = 5000;

  function item(e, kind, extra) {
    var o = { sel: sel(e), name: cut(nameOf(e), 60), role: kind };
    for (var k in extra) o[k] = extra[k];
    return o;
  }

  function walk(root) {
    var all = slice(root.querySelectorAll('*'));
    for (var i = 0; i < all.length && seen < CAP; i++, seen++) {
      var e = all[i], t = e.tagName;
      if (e.shadowRoot) walk(e.shadowRoot);
      if (t === 'IFRAME' || t === 'FRAME') {
        var d = null;
        try { d = e.contentDocument; } catch (x) { d = null; }
        if (d && d.documentElement) { frames++; walk(d); }
        continue;
      }
      if (/^(SCRIPT|STYLE|NOSCRIPT|TEMPLATE|HEAD)$/.test(t)) continue;
      if (/^H[1-3]$/.test(t)) { if (vis(e) && norm(e.innerText)) heads.push({ lvl: t.toLowerCase(), name: cut(e.innerText, 90) }); continue; }
      var role = roleOf(e);
      if (!role) continue;
      if (!vis(e) && !(role === 'checkbox' || role === 'radio')) continue;
      var dis = !!(e.disabled || e.getAttribute('aria-disabled') === 'true');
      if (role === 'link') { links.push(item(e, 'link', { dis: dis })); continue; }
      if (role === 'button' || role === 'clickable' || role === 'tab' || role === 'menuitem') { buttons.push(item(e, role === 'clickable' ? 'button' : role, { dis: dis })); continue; }
      var ex = { dis: dis };
      if (e.required) ex.req = true;
      if (e.readOnly) ex.ro = true;
      if (role === 'checkbox' || role === 'radio') ex.checked = !!e.checked;
      if (role === 'select') {
        ex.opts = slice(e.options).slice(0, 8).map(function (o) { return cut(o.label || o.text, 24); });
        ex.more = Math.max(0, e.options.length - 8);
        ex.now = e.selectedIndex >= 0 ? cut(e.options[e.selectedIndex].label || e.options[e.selectedIndex].text, 24) : '';
      }
      if (/^(textbox|email|search|tel|url|number|date|time|datetime-local|month|week|color|range)$/.test(role) && !secret(e)) {
        var v = String(e.value || '');
        if (v) ex.now = cut(v, 40);
      }
      if (secret(e)) ex.secret = true;
      if (e.placeholder && !labelOf(e).length) ex.ph = cut(e.placeholder, 30);
      fields.push(item(e, role, ex));
    }
  }
  walk(S.base);

  var out = [], used = 0;
  function line(s) { if (used + s.length + 1 > budget) return false; out.push(s); used += s.length + 1; return true; }
  function section(title, list, limit, fmt) {
    if (!list.length) return;
    if (!line(title)) return;
    var shown = 0;
    for (var i = 0; i < list.length && i < limit; i++) { if (!line('  ' + fmt(list[i]))) break; shown++; }
    if (shown < list.length) line('  (' + (list.length - shown) + ' more not shown; read a part of the page, or scroll, to see them)');
  }
  var p = S.page();
  line('PAGE ' + JSON.stringify(cut(p.title, 80)) + '  ' + p.url + '  (' + p.ready + (p.status ? ', HTTP ' + p.status : '') + ')  scrolled ' + p.y + ' of ' + Math.max(0, p.h - p.vh) + ' px');
  section('HEADINGS', heads, 8, function (h) { return h.lvl + ' ' + h.name; });
  section('FIELDS', fields, 30, function (f) {
    var s = f.role + (f.name ? ' ' + JSON.stringify(f.name) : '') + '  ' + f.sel;
    if (f.secret) return s + '  [a password or card field: the person types it, Syn does not]';
    var bits = [];
    if (f.opts) bits.push('options: ' + f.opts.join(' | ') + (f.more ? ' | +' + f.more + ' more' : ''));
    if (f.now) bits.push('now: ' + f.now);
    if (f.checked != null) bits.push(f.checked ? 'checked' : 'unchecked');
    if (f.ph) bits.push('placeholder: ' + f.ph);
    if (f.req) bits.push('required');
    if (f.ro) bits.push('read-only');
    if (f.dis) bits.push('disabled');
    return s + (bits.length ? '  [' + bits.join('; ') + ']' : '');
  });
  section('BUTTONS', buttons, 25, function (b) { return b.role + ' ' + JSON.stringify(b.name) + '  ' + b.sel + (b.dis ? '  [disabled]' : ''); });
  section('LINKS', links, 40, function (l) { return 'link ' + JSON.stringify(l.name || '(no text)') + '  ' + l.sel; });
  if (frames) line(frames + ' frame(s) are included above; a selector into one reads  frameSelector' + DEEP + 'inner');
  return { ok: true, text: out.join('\n'), counts: { fields: fields.length, buttons: buttons.length, links: links.length } };
};

S.find = function (text) {
  var w = norm(text).toLowerCase();
  if (!w) return { ok: false, why: 'empty', say: 'find: give the words to look for in `text`' };
  var all = slice(S.base.querySelectorAll('*')), hits = [], hidden = 0;
  for (var i = 0; i < all.length; i++) {
    var e = all[i];
    if (/^(SCRIPT|STYLE|NOSCRIPT|HEAD|META|LINK|TITLE|TEMPLATE)$/.test(e.tagName)) continue;
    var tc = e.textContent || '';
    if (tc.toLowerCase().indexOf(w) < 0) continue;
    // Only where the words are written: an element none of whose children
    // also hold them.
    var inner = false;
    for (var c = e.firstElementChild; c; c = c.nextElementSibling) { if ((c.textContent || '').toLowerCase().indexOf(w) >= 0) { inner = true; break; } }
    if (inner) continue;
    if (!vis(e)) { hidden++; continue; }
    hits.push(e);
  }
  return {
    ok: true, total: hits.length, hidden: hidden,
    hits: hits.slice(0, 10).map(function (e) {
      var t = norm(e.innerText || e.textContent), at = t.toLowerCase().indexOf(w);
      var from = Math.max(0, at - 40);
      return { sel: sel(e), tag: e.tagName.toLowerCase(), snip: (from ? '…' : '') + t.slice(from, at + w.length + 60) + (t.length > at + w.length + 60 ? '…' : '') };
    })
  };
};

// ---------------------------------------------------------------- actions

// The thing to press or hover: found, shown, enabled, and not covered. The
// point is in the top window's own coordinates, which is what a real mouse
// event takes.
S.prep = function (selector, mode) {
  var r = resolve(selector, true), e = r.el, d = describe(e);
  if (mode === 'click' && e.tagName === 'SELECT') return fail('select', d + ' is a drop-down list. Choose an option with action select and the option\'s text in `text`.');
  if (mode === 'click' && e.tagName === 'INPUT' && e.type === 'file') return fail('file', d + ' opens a file chooser, which Syn cannot drive. Ask the person to choose the file in the browser window.');
  if (!vis(e)) return fail('hidden', d + many(r) + ' is not visible: it is hidden, has no size, or sits in a menu that is closed. Open or reveal it first (hover or press what shows it), then try again.');
  if (e.disabled || e.getAttribute('aria-disabled') === 'true') return fail('disabled', d + ' is disabled: it cannot be pressed until the page enables it.');
  e.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
  var rects = slice(e.getClientRects()).filter(function (q) { return q.width > 0 && q.height > 0; });
  var q = rects[0] || e.getBoundingClientRect();
  var w = e.ownerDocument.defaultView;
  var vw = w.innerWidth, vh = w.innerHeight;
  var left = Math.max(q.left, 0), right = Math.min(q.right, vw), top = Math.max(q.top, 0), bottom = Math.min(q.bottom, vh);
  var cx = (left + right) / 2, cy = (top + bottom) / 2;
  var root = e.getRootNode();
  var hit = (root && root.elementFromPoint ? root : e.ownerDocument).elementFromPoint(cx, cy);
  while (hit && hit.shadowRoot && hit.shadowRoot.elementFromPoint) {
    var h2 = hit.shadowRoot.elementFromPoint(cx, cy);
    if (!h2 || h2 === hit) break;
    hit = h2;
  }
  var lab = hit && hit.closest ? hit.closest('label') : null;
  var good = hit && (hit === e || e.contains(hit) || (lab && lab.control === e) || (e.closest && e.closest('label') && e.closest('label').contains(hit)));
  if (!good) return fail('covered', d + many(r) + ' is covered by ' + describe(hit) + ' at the point where it would be pressed. Close, dismiss or scroll past whatever is in front of it first.');
  var x = cx, y = cy;
  while (w && w.frameElement) {
    var f = w.frameElement, fr = f.getBoundingClientRect();
    x += fr.left + f.clientLeft; y += fr.top + f.clientTop;
    w = w.parent;
  }
  return {
    ok: true, x: Math.round(x * 10) / 10, y: Math.round(y * 10) / 10, d: d, n: r.n, sel: sel(e),
    tag: e.tagName.toLowerCase(), type: e.type || '', checked: e.checked
  };
};

// What an element looks like now, for saying what a press did.
S.state = function (selector) {
  try {
    var e = resolve(selector, false).el;
    var o = { ok: true, d: describe(e) };
    if (e.type === 'checkbox' || e.type === 'radio') o.checked = !!e.checked;
    if (e.tagName === 'SELECT' && e.selectedIndex >= 0) o.now = cut(e.options[e.selectedIndex].label || e.options[e.selectedIndex].text, 40);
    if (/^(INPUT|TEXTAREA)$/.test(e.tagName) && !secret(e)) o.now = cut(e.value, 60);
    if (e.getAttribute('aria-expanded') != null) o.expanded = e.getAttribute('aria-expanded') === 'true';
    return o;
  } catch (x) { return { ok: false }; }
};

// A field made ready for typing: found, allowed, shown, focused.
S.field = function (selector, clear) {
  var r = resolve(selector, true), e = r.el, d = describe(e);
  var kind = (e.tagName === 'INPUT' || e.tagName === 'TEXTAREA') ? 'field' : (editable(e) ? 'editor' : '');
  if (!kind) {
    return fail('notfield', d + ' is not something text can be typed into. Find the field with read{"selector":":map"} and use its selector.');
  }
  if (kind === 'field' && /^(checkbox|radio|submit|button|reset|file|image|range|color)$/.test(e.type)) {
    return fail('notfield', d + ' is a ' + e.type + ' control, not a text field. Press it with action click.');
  }
  var why = secret(e);
  if (why) return fail('secret', d + ' is ' + why + '. Syn does not type those: ask the person to enter it themselves in the browser window, then carry on.');
  if (!vis(e)) return fail('hidden', d + many(r) + ' is not visible, so it cannot be typed into. Reveal it first.');
  if (e.disabled || e.readOnly) return fail('disabled', d + ' is ' + (e.disabled ? 'disabled' : 'read-only') + '.');
  e.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
  e.focus({ preventScroll: true });
  if (clear) {
    if (kind === 'field' && e.select) e.select();
    else { var rg = e.ownerDocument.createRange(); rg.selectNodeContents(e); var sl = e.ownerDocument.defaultView.getSelection(); sl.removeAllRanges(); sl.addRange(rg); }
  } else if (kind === 'field' && e.setSelectionRange && /^(text|search|url|tel|password|textarea)$/.test(e.type || 'textarea')) {
    try { var n = e.value.length; e.setSelectionRange(n, n); } catch (x) { /* some types have no caret */ }
  }
  return { ok: true, d: d, n: r.n, kind: kind, sel: sel(e) };
};

// Set a field's whole value the way a page's own scripts expect to hear it
// (a frameworks's controlled input ignores a plain assignment).
S.write = function (selector, value) {
  var r = resolve(selector, true), e = r.el, d = describe(e);
  var why = secret(e);
  if (why) return fail('secret', d + ' is ' + why + '. Syn does not fill those in: ask the person to enter it in the browser window, then carry on.');
  if (e.tagName === 'SELECT') return S.choose(selector, value);
  if (e.tagName === 'INPUT' && (e.type === 'checkbox' || e.type === 'radio')) {
    var want = /^(1|true|yes|on|checked)$/i.test(String(value).trim());
    if (!!e.checked !== want) e.click();
    return { ok: true, d: d, now: e.checked ? 'checked' : 'unchecked', same: true };
  }
  if (e.tagName === 'INPUT' || e.tagName === 'TEXTAREA') {
    if (e.disabled || e.readOnly) return fail('disabled', d + ' is ' + (e.disabled ? 'disabled' : 'read-only') + '.');
    var proto = Object.getPrototypeOf(e), desc = Object.getOwnPropertyDescriptor(proto, 'value');
    if (desc && desc.set) desc.set.call(e, value); else e.value = value;
    e.dispatchEvent(new Event('input', { bubbles: true }));
    e.dispatchEvent(new Event('change', { bubbles: true }));
    var got = String(e.value);
    return { ok: true, d: d, now: cut(got, 80), same: got === value, n: r.n };
  }
  if (editable(e)) {
    e.focus({ preventScroll: true });
    var rg = e.ownerDocument.createRange(); rg.selectNodeContents(e);
    var sl = e.ownerDocument.defaultView.getSelection(); sl.removeAllRanges(); sl.addRange(rg);
    var done = e.ownerDocument.execCommand('insertText', false, value);
    if (!done) { e.textContent = value; e.dispatchEvent(new Event('input', { bubbles: true })); }
    return { ok: true, d: d, now: cut(e.innerText, 80), same: norm(e.innerText) === norm(value), n: r.n };
  }
  // Not a field: the old behaviour, replace what the page shows. Said so.
  e.innerText = value;
  return { ok: true, d: d, now: cut(value, 80), same: true, note: 'it was not a field, so its visible text was replaced' };
};

S.choose = function (selector, want) {
  var r = resolve(selector, true), e = r.el, d = describe(e);
  if (e.tagName !== 'SELECT') return fail('notselect', d + ' is not a drop-down list. To choose in a custom menu, press it and then press the option.');
  var w = norm(want).toLowerCase();
  var opts = slice(e.options);
  var by = function (f) { return opts.filter(f); };
  var hit = by(function (o) { return o.value.toLowerCase() === w; });
  if (!hit.length) hit = by(function (o) { return norm(o.label || o.text).toLowerCase() === w; });
  if (!hit.length && w) hit = by(function (o) { return norm(o.label || o.text).toLowerCase().indexOf(w) >= 0; });
  if (!hit.length) return fail('nooption', d + ' has no option "' + want + '". Its options are: ' + opts.slice(0, 12).map(function (o) { return JSON.stringify(norm(o.label || o.text)); }).join(', ') + (opts.length > 12 ? ', and ' + (opts.length - 12) + ' more' : ''));
  if (hit.length > 1) return fail('manyoptions', d + ' has ' + hit.length + ' options matching "' + want + '": ' + hit.slice(0, 6).map(function (o) { return JSON.stringify(norm(o.label || o.text)); }).join(', ') + '. Say which.');
  if (hit[0].disabled) return fail('disabled', 'the option "' + norm(hit[0].label || hit[0].text) + '" is disabled.');
  e.value = hit[0].value;
  e.dispatchEvent(new Event('input', { bubbles: true }));
  e.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: true, d: d, now: norm(hit[0].label || hit[0].text), same: true, n: r.n };
};

S.scroll = function (selector, dir) {
  var d = String(dir || '').toLowerCase(), s = String(selector || '').trim();
  var el = null;
  if (s && s !== ':doc') { var r = resolve(s, false); el = r.el; }
  var page = !el || el === document.documentElement || el === document.body;
  if (el && !page && !d) {
    el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
    return { ok: true, said: 'scrolled ' + describe(el) + ' into view', y: Math.round(window.scrollY), h: S.page().h, vh: window.innerHeight };
  }
  var box = page ? window : el;
  var step = (page ? window.innerHeight : el.clientHeight) * 0.85;
  var before = page ? window.scrollY : el.scrollTop;
  if (d === 'top') box.scrollTo(0, 0);
  else if (d === 'bottom') box.scrollTo(0, 1e9);
  else if (d === 'up') box.scrollBy(0, -step);
  else if (d === 'down' || d === '') box.scrollBy(0, step);
  else return fail('direction', 'scroll: the direction is down, up, top or bottom, not ' + JSON.stringify(dir));
  var after = page ? window.scrollY : el.scrollTop;
  var max = page ? Math.max(document.documentElement.scrollHeight, document.body.scrollHeight) - window.innerHeight : el.scrollHeight - el.clientHeight;
  return { ok: true, said: (Math.round(after) === Math.round(before) ? 'did not move: already at the ' + (d === 'up' || d === 'top' ? 'top' : 'bottom') : 'scrolled ' + (page ? 'the page' : describe(el)) + ' to ' + Math.round(after) + ' of ' + Math.round(Math.max(0, max)) + ' px'), y: Math.round(after), h: Math.round(max) };
};

S.check = function (selector, text, gone) {
  var met = true, why = '';
  if (selector) {
    var found = null;
    try { found = resolve(selector, false); } catch (x) { found = null; }
    var there = !!(found && vis(found.el));
    met = gone ? !there : there;
    why = there ? describe(found.el) : '';
  }
  if (met && text) {
    var has = norm(document.body ? document.body.innerText : '').toLowerCase().indexOf(norm(text).toLowerCase()) >= 0;
    met = gone ? !has : has;
  }
  return { met: met, why: why };
};

S.focus = function (selector) {
  var r = resolve(selector, true), e = r.el, d = describe(e);
  if (!vis(e)) return fail('hidden', d + many(r) + ' is not visible, so it cannot take focus. Reveal it first.');
  e.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
  e.focus({ preventScroll: true });
  return { ok: true, d: d, n: r.n };
};

// Cosmetic style on what the page shows; `pairs` is [[property, value], ...].
S.style = function (selector, pairs) {
  var r = resolve(selector, false), e = r.el;
  for (var i = 0; i < pairs.length; i++) e.style.setProperty(pairs[i][0], pairs[i][1]);
  return { ok: true, d: describe(e), n: pairs.length };
};

S.html = function (selector, off) {
  var r = resolve(selector, false), h = r.el.outerHTML || '';
  off = off | 0;
  return { ok: true, text: h.slice(off, off + MAX_TEXT), total: h.length, off: off };
};

S.focused = function () {
  var a = document.activeElement;
  return { d: a ? describe(a) : 'nothing', field: !!(a && (/^(INPUT|TEXTAREA)$/.test(a.tagName) || editable(a))), sel: a && a !== document.body ? sel(a) : '' };
};
