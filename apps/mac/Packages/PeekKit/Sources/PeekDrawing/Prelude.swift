import Foundation

/// The JavaScript prelude every drawing VM evaluates before the drawing (visual.md B3, BLUEPRINT §8.6).
///
/// It implements `peek` (`frame`, `on`, `log`), `Path2D` (including SVG path strings), `CanvasGradient`
/// and the `ctx` object: canvas state lives in JS, every call appends `[opcode, argc, args…]` to the shared
/// `Float64Array` (`__peek_ops`), strings go to a per-frame string table, and one native `__peek_flush`
/// call per frame hands the whole frame to Swift. Style ops are recorded only when a value changes.
///
/// The source is embedded (not a bundle resource) so PeekKit needs no resource bundle; the opcode table
/// is generated from ``OpCode`` so JS and Swift always agree.
enum Prelude {
    static let filename = "peek-prelude.js"

    static let source: String = {
        let table = OpCode.allCases.map { "\($0.jsName): \($0.rawValue)" }.joined(separator: ", ")
        return body
            .replacingOccurrences(of: "/*OPCODES*/", with: "{ \(table) }")
            .replacingOccurrences(of: "/*MAX_OPS*/", with: String(DrawingLimits.maxOpsPerFrame))
    }()

    // swiftlint:disable line_length
    private static let body = #"""
(function () {
  'use strict';
  const OPS = __peek_ops, FLUSH = __peek_flush, LOG = __peek_log, MEASURE = __peek_measure;
  const OP = /*OPCODES*/;
  const MAX_OPS = /*MAX_OPS*/;
  const CAP = OPS.length - 8;             // room for the frameInfo op
  const EVENTS = ['enter', 'leave', 'send', 'answer', 'click', 'move'];
  const isF = Number.isFinite;

  // ---- recording -----------------------------------------------------------
  let n = 0, opCount = 0, dropped = 0, full = false, inFrame = false;
  let strings = [];
  const stringIds = new Map();
  const warned = new Set();
  let logCount = 0, logLimit = 200;

  function warnOnce(key, message) {
    if (warned.has(key)) return;
    warned.add(key);
    LOG('warning: ' + message);
  }
  function str(s) {
    let i = stringIds.get(s);
    if (i === undefined) { i = strings.length; strings.push(s); stringIds.set(s, i); }
    return i;
  }
  function room(argc) {
    if (!inFrame) {
      warnOnce('outside', 'ctx was used outside peek.frame(); drawing calls only count inside the frame callback');
      return false;
    }
    if (full || opCount >= MAX_OPS || n + 2 + argc > CAP) { full = true; dropped++; return false; }
    return true;
  }
  function op0(c) { if (room(0)) { OPS[n++] = c; OPS[n++] = 0; opCount++; } }
  function op1(c, a) { if (room(1)) { OPS[n++] = c; OPS[n++] = 1; OPS[n++] = a; opCount++; } }
  function op2(c, a, b) { if (room(2)) { OPS[n++] = c; OPS[n++] = 2; OPS[n++] = a; OPS[n++] = b; opCount++; } }
  function op4(c, a, b, d, e) {
    if (room(4)) { OPS[n++] = c; OPS[n++] = 4; OPS[n++] = a; OPS[n++] = b; OPS[n++] = d; OPS[n++] = e; opCount++; }
  }
  function opv(c, args) {
    const k = args.length;
    if (room(k)) { OPS[n++] = c; OPS[n++] = k; for (let i = 0; i < k; i++) OPS[n++] = args[i]; opCount++; }
  }

  function describe(v) {
    if (v === null) return 'null';
    if (typeof v === 'string') return JSON.stringify(v);
    if (typeof v === 'object' || typeof v === 'function') return typeof v;
    return String(v);
  }
  function ruleCode(v, fn) {
    if (v === undefined || v === 'nonzero') return 0;
    if (v === 'evenodd') return 1;
    throw new TypeError(`${fn}: the fill rule must be 'nonzero' or 'evenodd', got ${describe(v)}`);
  }

  // ---- path commands (shared by ctx and Path2D) ------------------------------
  // Each returns the validated argument list, or null when canvas would ignore the call.
  function finite(args) { for (let i = 0; i < args.length; i++) if (!isF(args[i])) return null; return args; }
  function negative(fn, what, v) {
    throw new RangeError(`${fn}: the ${what} ${v} is negative (canvas IndexSizeError)`);
  }
  const PATH = {
    moveTo: (x, y) => finite([+x, +y]),
    lineTo: (x, y) => finite([+x, +y]),
    quadraticCurveTo: (a, b, x, y) => finite([+a, +b, +x, +y]),
    bezierCurveTo: (a, b, c, d, x, y) => finite([+a, +b, +c, +d, +x, +y]),
    arc(x, y, r, a0, a1, ccw) {
      const v = finite([+x, +y, +r, +a0, +a1]);
      if (v === null) return null;
      if (v[2] < 0) negative('arc', 'radius', v[2]);
      v.push(ccw ? 1 : 0);
      return v;
    },
    arcTo(x1, y1, x2, y2, r) {
      const v = finite([+x1, +y1, +x2, +y2, +r]);
      if (v !== null && v[4] < 0) negative('arcTo', 'radius', v[4]);
      return v;
    },
    ellipse(x, y, rx, ry, rot, a0, a1, ccw) {
      const v = finite([+x, +y, +rx, +ry, +rot, +a0, +a1]);
      if (v === null) return null;
      if (v[2] < 0) negative('ellipse', 'x radius', v[2]);
      if (v[3] < 0) negative('ellipse', 'y radius', v[3]);
      v.push(ccw ? 1 : 0);
      return v;
    },
    rect: (x, y, w, h) => finite([+x, +y, +w, +h]),
    roundRect(x, y, w, h, radii) {
      const v = finite([+x, +y, +w, +h]);
      if (v === null) return null;
      let list;
      if (radii === undefined) list = [0];
      else if (typeof radii === 'number' || (radii !== null && typeof radii === 'object' && !Array.isArray(radii) && typeof radii[Symbol.iterator] !== 'function')) list = [radii];
      else if (radii !== null && typeof radii[Symbol.iterator] === 'function') list = Array.from(radii);
      else list = [+radii];
      if (list.length < 1 || list.length > 4) throw new RangeError(`roundRect: expected 1 to 4 radii, got ${list.length}`);
      const pts = [];
      for (const r of list) {
        let rx, ry;
        if (r !== null && typeof r === 'object') { rx = +(r.x ?? 0); ry = +(r.y ?? 0); } else { rx = ry = +r; }
        if (!isF(rx) || !isF(ry)) return null;
        if (rx < 0 || ry < 0) negative('roundRect', 'radius', Math.min(rx, ry));
        pts.push([rx, ry]);
      }
      let tl, tr, br, bl;
      switch (pts.length) {
        case 1: tl = tr = br = bl = pts[0]; break;
        case 2: tl = br = pts[0]; tr = bl = pts[1]; break;
        case 3: tl = pts[0]; tr = bl = pts[1]; br = pts[2]; break;
        default: [tl, tr, br, bl] = pts;
      }
      v.push(tl[0], tl[1], tr[0], tr[1], br[0], br[1], bl[0], bl[1]);
      return v;
    },
    closePath: () => [],
  };

  // ---- Path2D -----------------------------------------------------------------
  const PATHS = new WeakMap();
  function pathData(p, fn) {
    const d = PATHS.get(p);
    if (d === undefined) throw new TypeError(`${fn}: expected a Path2D`);
    return d;
  }
  function record(d, code, args) {
    d.c.push(code, args.length);
    for (let i = 0; i < args.length; i++) d.c.push(args[i]);
    d.k++;
  }

  class Path2D {
    constructor(init) {
      const d = { c: [], k: 0 };
      PATHS.set(this, d);
      if (init instanceof Path2D) {
        const o = pathData(init, 'Path2D');
        d.c = o.c.slice(); d.k = o.k;
      } else if (init !== undefined) {
        parseSVG(String(init), this);
      }
    }
    addPath(path, m) {
      const d = pathData(this, 'Path2D.addPath'), o = pathData(path, 'Path2D.addPath');
      let push = null;
      if (m !== undefined && m !== null) {
        push = finite([+(m.a ?? 1), +(m.b ?? 0), +(m.c ?? 0), +(m.d ?? 1), +(m.e ?? 0), +(m.f ?? 0)]);
        if (push === null) return;
      }
      if (push) record(d, OP.pathPushTransform, push);
      for (let i = 0; i < o.c.length; i++) d.c.push(o.c[i]);
      d.k += o.k;
      if (push) record(d, OP.pathPopTransform, []);
    }
  }
  for (const name of Object.keys(PATH)) {
    const check = PATH[name], code = OP[name];
    Object.defineProperty(Path2D.prototype, name, {
      value: function (...args) { const v = check(...args); if (v !== null) record(pathData(this, 'Path2D.' + name), code, v); },
      writable: true, configurable: true,
    });
  }

  // SVG path data (https://www.w3.org/TR/SVG/paths.html) → canvas commands. Keeps everything up to the
  // first error, like browsers do.
  function parseSVG(src, path) {
    let i = 0, cx = 0, cy = 0, sx = 0, sy = 0, lastCtl = null, lastCmd = '';
    const len = src.length;
    function wsp() { while (i < len && ' \t\n\r\f,'.includes(src[i])) i++; }
    function number() {
      wsp();
      const m = /^[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?/.exec(src.slice(i, i + 64));
      if (m === null) throw new SyntaxError(`expected a number at offset ${i}`);
      i += m[0].length;
      return parseFloat(m[0]);
    }
    function flag() {
      wsp();
      const ch = src[i];
      if (ch !== '0' && ch !== '1') throw new SyntaxError(`expected an arc flag (0 or 1) at offset ${i}`);
      i++;
      return ch === '1';
    }
    function more() { wsp(); return i < len && /[-+.\d]/.test(src[i]); }
    function arcTo(rx, ry, rot, large, sweep, x, y) {
      if (x === cx && y === cy) return;
      rx = Math.abs(rx); ry = Math.abs(ry);
      if (rx === 0 || ry === 0) { path.lineTo(x, y); return; }
      const phi = rot * Math.PI / 180, cp = Math.cos(phi), sp = Math.sin(phi);
      const dx2 = (cx - x) / 2, dy2 = (cy - y) / 2;
      const x1 = cp * dx2 + sp * dy2, y1 = -sp * dx2 + cp * dy2;
      const lam = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
      if (lam > 1) { const s = Math.sqrt(lam); rx *= s; ry *= s; }
      const num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
      const den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
      let coef = den === 0 ? 0 : Math.sqrt(Math.max(0, num / den));
      if (large === sweep) coef = -coef;
      const ccx = coef * (rx * y1) / ry, ccy = coef * -(ry * x1) / rx;
      const ex = cp * ccx - sp * ccy + (cx + x) / 2, ey = sp * ccx + cp * ccy + (cy + y) / 2;
      const ang = (ux, uy, vx, vy) => Math.atan2(ux * vy - uy * vx, ux * vx + uy * vy);
      const ux = (x1 - ccx) / rx, uy = (y1 - ccy) / ry, vx = (-x1 - ccx) / rx, vy = (-y1 - ccy) / ry;
      const t1 = ang(1, 0, ux, uy);
      let dt = ang(ux, uy, vx, vy);
      if (!sweep && dt > 0) dt -= 2 * Math.PI;
      else if (sweep && dt < 0) dt += 2 * Math.PI;
      path.ellipse(ex, ey, rx, ry, phi, t1, t1 + dt, !sweep);
    }
    try {
      wsp();
      while (i < len) {
        let cmd = src[i];
        if (/[MmZzLlHhVvCcSsQqTtAa]/.test(cmd)) { i++; }
        else if (lastCmd !== '' && /[-+.\d]/.test(cmd) && !/[Zz]/.test(lastCmd)) { cmd = lastCmd === 'M' ? 'L' : lastCmd === 'm' ? 'l' : lastCmd; }
        else throw new SyntaxError(`unexpected ${JSON.stringify(cmd)} at offset ${i}`);
        const rel = cmd === cmd.toLowerCase();
        const C = cmd.toUpperCase();
        let ctl = null;
        switch (C) {
          case 'M': {
            let x = number(), y = number();
            if (rel) { x += cx; y += cy; }
            path.moveTo(x, y); cx = sx = x; cy = sy = y;
            while (more()) { x = number(); y = number(); if (rel) { x += cx; y += cy; } path.lineTo(x, y); cx = x; cy = y; }
            break;
          }
          case 'Z': path.closePath(); cx = sx; cy = sy; break;
          case 'L': do { let x = number(), y = number(); if (rel) { x += cx; y += cy; } path.lineTo(x, y); cx = x; cy = y; } while (more()); break;
          case 'H': do { let x = number(); if (rel) x += cx; path.lineTo(x, cy); cx = x; } while (more()); break;
          case 'V': do { let y = number(); if (rel) y += cy; path.lineTo(cx, y); cy = y; } while (more()); break;
          case 'C': do {
            let a = number(), b = number(), c = number(), d = number(), x = number(), y = number();
            if (rel) { a += cx; b += cy; c += cx; d += cy; x += cx; y += cy; }
            path.bezierCurveTo(a, b, c, d, x, y); ctl = ['C', c, d]; cx = x; cy = y;
          } while (more()); break;
          case 'S': do {
            let c = number(), d = number(), x = number(), y = number();
            if (rel) { c += cx; d += cy; x += cx; y += cy; }
            const prev = ctl ?? lastCtl;
            const a = prev && prev[0] === 'C' ? 2 * cx - prev[1] : cx, b = prev && prev[0] === 'C' ? 2 * cy - prev[2] : cy;
            path.bezierCurveTo(a, b, c, d, x, y); ctl = ['C', c, d]; cx = x; cy = y;
          } while (more()); break;
          case 'Q': do {
            let a = number(), b = number(), x = number(), y = number();
            if (rel) { a += cx; b += cy; x += cx; y += cy; }
            path.quadraticCurveTo(a, b, x, y); ctl = ['Q', a, b]; cx = x; cy = y;
          } while (more()); break;
          case 'T': do {
            let x = number(), y = number();
            if (rel) { x += cx; y += cy; }
            const prev = ctl ?? lastCtl;
            const a = prev && prev[0] === 'Q' ? 2 * cx - prev[1] : cx, b = prev && prev[0] === 'Q' ? 2 * cy - prev[2] : cy;
            path.quadraticCurveTo(a, b, x, y); ctl = ['Q', a, b]; cx = x; cy = y;
          } while (more()); break;
          case 'A': do {
            const rx = number(), ry = number(), rot = number(), large = flag(), sweep = flag();
            let x = number(), y = number();
            if (rel) { x += cx; y += cy; }
            arcTo(rx, ry, rot, large, sweep, x, y); cx = x; cy = y;
          } while (more()); break;
        }
        lastCtl = ctl;
        lastCmd = cmd;
        wsp();
      }
    } catch (e) {
      if (!(e instanceof SyntaxError)) throw e;
      warnOnce('svg:' + src.slice(0, 40), `Path2D: the SVG path data is invalid (${e.message}); drawing it up to that point`);
    }
  }

  // ---- gradients ----------------------------------------------------------------
  const GRADIENTS = new WeakMap();
  class CanvasGradient {
    constructor() { throw new TypeError('use ctx.createLinearGradient, ctx.createRadialGradient or ctx.createConicGradient'); }
    addColorStop(offset, color) {
      const g = GRADIENTS.get(this);
      if (g === undefined) throw new TypeError('addColorStop: expected a CanvasGradient');
      offset = +offset;
      if (!(offset >= 0 && offset <= 1)) throw new RangeError(`addColorStop: the offset ${offset} is outside 0…1 (canvas IndexSizeError)`);
      g.stops.push(offset, String(color));
      g.version++;
    }
  }
  function makeGradient(kind, params) {
    const g = Object.create(CanvasGradient.prototype);
    GRADIENTS.set(g, { kind, params, stops: [], version: 0 });
    return g;
  }
  function emitGradient(code, g) {
    const args = [g.kind, ...g.params];
    args.push(g.stops.length / 2);
    for (let i = 0; i < g.stops.length; i += 2) args.push(g.stops[i], str(g.stops[i + 1]));
    opv(code, args);
  }

  // ---- canvas state ------------------------------------------------------------
  const CAPS = { butt: 0, round: 1, square: 2 };
  const JOINS = { miter: 0, round: 1, bevel: 2 };
  const COMPOSITES = { 'source-over': 0, multiply: 1, screen: 2, overlay: 3, 'destination-out': 4, lighter: 5 };
  const ALIGNS = { start: 0, end: 1, left: 2, right: 3, center: 4 };
  const BASELINES = { alphabetic: 0, top: 1, hanging: 2, middle: 3, ideographic: 4, bottom: 5 };
  const MATERIALS = { hud: 0, popover: 1, menu: 2, sidebar: 3, underWindow: 4 };

  function defaults() {
    return {
      fillStyle: '#000000', fillVersion: -1, strokeStyle: '#000000', strokeVersion: -1,
      lineWidth: 1, lineCap: 'butt', lineJoin: 'miter', miterLimit: 10, lineDash: [], lineDashOffset: 0,
      globalAlpha: 1, globalCompositeOperation: 'source-over',
      shadowColor: 'rgba(0, 0, 0, 0)', shadowBlur: 0, shadowOffsetX: 0, shadowOffsetY: 0,
      filter: 'none', font: '10px sans-serif', textAlign: 'start', textBaseline: 'alphabetic',
      m: [1, 0, 0, 1, 0, 0],
    };
  }
  let S = defaults();
  const stack = [];
  let vibrant = false;

  function copyState(s) { const c = Object.assign({}, s); c.lineDash = s.lineDash.slice(); c.m = s.m.slice(); return c; }
  function setPaint(which, v) {
    const key = which === 'fill' ? 'fillStyle' : 'strokeStyle', ver = which === 'fill' ? 'fillVersion' : 'strokeVersion';
    const g = v !== null && typeof v === 'object' ? GRADIENTS.get(v) : undefined;
    if (g !== undefined) {
      if (S[key] !== v || S[ver] !== g.version) {
        S[key] = v; S[ver] = g.version;
        emitGradient(which === 'fill' ? OP.fillGradient : OP.strokeGradient, g);
      }
      return;
    }
    if (v !== null && typeof v === 'object') { warnOnce('pattern', `${key} only accepts CSS colors and gradients (patterns are not supported)`); return; }
    const s = String(v);
    if (S[key] !== s) { S[key] = s; S[ver] = -1; op1(which === 'fill' ? OP.fillColor : OP.strokeColor, str(s)); }
  }
  function setNumber(key, code, v, ok) {
    v = +v;
    if (!isF(v) || !ok(v)) return;
    if (S[key] !== v) { S[key] = v; op1(code, v); }
  }
  function setEnum(key, code, table, v) {
    const c = table[v];
    if (c === undefined) {
      warnOnce(key + ':' + v, `${key} ${describe(v)} is not supported; keeping ${JSON.stringify(S[key])}`);
      return;
    }
    if (S[key] !== v) { S[key] = v; op1(code, c); }
  }
  function multiply(m, a, b, c, d, e, f) {
    const [ma, mb, mc, md, me, mf] = m;
    m[0] = ma * a + mc * b; m[1] = mb * a + md * b;
    m[2] = ma * c + mc * d; m[3] = mb * c + md * d;
    m[4] = ma * e + mc * f + me; m[5] = mb * e + md * f + mf;
  }
  function emitPath(p, following) {
    const d = pathData(p, 'ctx');
    if (!inFrame) { room(0); return false; }
    const need = d.c.length + 6 + following, count = d.k + 3;
    if (full || opCount + count > MAX_OPS || n + need > CAP) { full = true; dropped += count; return false; }
    OPS[n++] = OP.path2DBegin; OPS[n++] = 0;
    const c = d.c;
    for (let i = 0; i < c.length; i++) OPS[n++] = c[i];
    OPS[n++] = OP.path2DEnd; OPS[n++] = 0;
    opCount += d.k + 2;
    return true;
  }
  function unsupported(name, hint) {
    return function () { throw new TypeError(`ctx.${name} is not supported in peek drawings${hint ? ': ' + hint : ''} (visual.md A5)`); };
  }

  const measureCache = new Map();
  function measure(font, text) {
    const key = font + '\u0000' + text;
    let m = measureCache.get(key);
    if (m === undefined) {
      if (measureCache.size > 512) measureCache.clear();
      m = MEASURE(font, text);
      measureCache.set(key, m);
    }
    return m;
  }

  const ctx = {
    canvas: Object.freeze({ width: 100, height: 100 }),
    imageSmoothingEnabled: true,
    imageSmoothingQuality: 'high',
    letterSpacing: '0px',
    direction: 'ltr',

    save() { stack.push(copyState(S)); op0(OP.save); },
    restore() { if (stack.length > 0) { S = stack.pop(); op0(OP.restore); } },
    reset() { S = defaults(); stack.length = 0; op0(OP.reset); },

    translate(x, y) { x = +x; y = +y; if (!isF(x) || !isF(y)) return; multiply(S.m, 1, 0, 0, 1, x, y); op2(OP.translate, x, y); },
    rotate(a) { a = +a; if (!isF(a)) return; const c = Math.cos(a), s = Math.sin(a); multiply(S.m, c, s, -s, c, 0, 0); op1(OP.rotate, a); },
    scale(x, y) { x = +x; y = +y; if (!isF(x) || !isF(y)) return; multiply(S.m, x, 0, 0, y, 0, 0); op2(OP.scale, x, y); },
    transform(a, b, c, d, e, f) {
      const v = finite([+a, +b, +c, +d, +e, +f]);
      if (v === null) return;
      multiply(S.m, ...v); opv(OP.transform, v);
    },
    setTransform(a, b, c, d, e, f) {
      let v;
      if (a === undefined) v = [1, 0, 0, 1, 0, 0];
      else if (a !== null && typeof a === 'object') v = finite([+(a.a ?? 1), +(a.b ?? 0), +(a.c ?? 0), +(a.d ?? 1), +(a.e ?? 0), +(a.f ?? 0)]);
      else v = finite([+a, +b, +c, +d, +e, +f]);
      if (v === null) return;
      S.m = v.slice(); opv(OP.setTransform, v);
    },
    resetTransform() { S.m = [1, 0, 0, 1, 0, 0]; op0(OP.resetTransform); },
    getTransform() { const [a, b, c, d, e, f] = S.m; return { a, b, c, d, e, f, is2D: true, isIdentity: a === 1 && b === 0 && c === 0 && d === 1 && e === 0 && f === 0 }; },

    beginPath() { op0(OP.beginPath); },
    closePath() { op0(OP.closePath); },
    moveTo(x, y) { x = +x; y = +y; if (isF(x) && isF(y)) op2(OP.moveTo, x, y); },
    lineTo(x, y) { x = +x; y = +y; if (isF(x) && isF(y)) op2(OP.lineTo, x, y); },
    arc(x, y, r, a0, a1, ccw) {
      x = +x; y = +y; r = +r; a0 = +a0; a1 = +a1;
      if (!(isF(x) && isF(y) && isF(r) && isF(a0) && isF(a1))) return;
      if (r < 0) negative('arc', 'radius', r);
      if (room(6)) { OPS[n++] = OP.arc; OPS[n++] = 6; OPS[n++] = x; OPS[n++] = y; OPS[n++] = r; OPS[n++] = a0; OPS[n++] = a1; OPS[n++] = ccw ? 1 : 0; opCount++; }
    },
    arcTo(...a) { const v = PATH.arcTo(...a); if (v !== null) opv(OP.arcTo, v); },
    ellipse(...a) { const v = PATH.ellipse(...a); if (v !== null) opv(OP.ellipse, v); },
    rect(x, y, w, h) { rect4(OP.rect, x, y, w, h); },
    roundRect(...a) { const v = PATH.roundRect(...a); if (v !== null) opv(OP.roundRect, v); },
    quadraticCurveTo(a, b, x, y) { rect4(OP.quadraticCurveTo, a, b, x, y); },
    bezierCurveTo(...a) { const v = PATH.bezierCurveTo(...a); if (v !== null) opv(OP.bezierCurveTo, v); },

    fill(a, b) {
      if (a instanceof Path2D) { const r = ruleCode(b, 'fill'); if (emitPath(a, 2)) op2(OP.fill, r, 1); }
      else op2(OP.fill, ruleCode(a, 'fill'), 0);
    },
    stroke(p) {
      if (p instanceof Path2D) { if (emitPath(p, 1)) op1(OP.stroke, 1); }
      else if (p === undefined) op1(OP.stroke, 0);
      else throw new TypeError(`stroke: expected a Path2D or nothing, got ${describe(p)}`);
    },
    clip(a, b) {
      if (a instanceof Path2D) { const r = ruleCode(b, 'clip'); if (emitPath(a, 2)) op2(OP.clip, r, 1); }
      else op2(OP.clip, ruleCode(a, 'clip'), 0);
    },
    fillRect(x, y, w, h) { rect4(OP.fillRect, x, y, w, h); },
    strokeRect(x, y, w, h) { rect4(OP.strokeRect, x, y, w, h); },
    clearRect(x, y, w, h) { rect4(OP.clearRect, x, y, w, h); },

    fillText(t, x, y, w) { text(OP.fillText, t, x, y, w); },
    strokeText(t, x, y, w) { text(OP.strokeText, t, x, y, w); },
    measureText(t) {
      const m = measure(S.font, String(t).replace(/[\t\n\f\r]/g, ' '));
      const w = m[0];
      const shift = S.textAlign === 'center' ? w / 2 : (S.textAlign === 'right' || S.textAlign === 'end') ? w : 0;
      return {
        width: w,
        actualBoundingBoxLeft: m[1] + shift, actualBoundingBoxRight: m[2] - shift,
        actualBoundingBoxAscent: m[3], actualBoundingBoxDescent: m[4],
        fontBoundingBoxAscent: m[5], fontBoundingBoxDescent: m[6],
      };
    },

    drawImage(img, a1, a2, a3, a4, a5, a6, a7, a8) {
      const argc = arguments.length;
      if (img === null || typeof img !== 'object' || !isF(img.id)) {
        throw new TypeError(`drawImage: expected an image handle from input.show or input.ask, got ${describe(img)}`);
      }
      const iw = +img.width, ih = +img.height;
      let v;
      if (argc === 3) v = [0, 0, iw, ih, +a1, +a2, iw, ih];
      else if (argc === 5) v = [0, 0, iw, ih, +a1, +a2, +a3, +a4];
      else if (argc === 9) v = [+a1, +a2, +a3, +a4, +a5, +a6, +a7, +a8];
      else throw new TypeError(`drawImage: takes 3, 5 or 9 arguments, got ${argc}`);
      if (finite(v) === null || !isF(iw) || !isF(ih)) return;
      opv(OP.drawImage, [img.id, iw, ih, ...v]);
    },

    createLinearGradient(x0, y0, x1, y1) {
      const v = finite([+x0, +y0, +x1, +y1]);
      if (v === null) throw new TypeError('createLinearGradient: every argument must be a finite number');
      return makeGradient(0, [...v, 0, 0]);
    },
    createRadialGradient(x0, y0, r0, x1, y1, r1) {
      const v = finite([+x0, +y0, +r0, +x1, +y1, +r1]);
      if (v === null) throw new TypeError('createRadialGradient: every argument must be a finite number');
      if (v[2] < 0) negative('createRadialGradient', 'start radius', v[2]);
      if (v[5] < 0) negative('createRadialGradient', 'end radius', v[5]);
      return makeGradient(1, v);
    },
    createConicGradient(a, x, y) {
      const v = finite([+a, +x, +y]);
      if (v === null) throw new TypeError('createConicGradient: every argument must be a finite number');
      return makeGradient(2, [...v, 0, 0, 0]);
    },

    setLineDash(segments) {
      if (segments === null || typeof segments !== 'object' || typeof segments[Symbol.iterator] !== 'function') {
        throw new TypeError('setLineDash: expected an array of numbers');
      }
      let list = Array.from(segments, Number);
      if (list.some(v => !isF(v) || v < 0)) return;
      if (list.length % 2 === 1) list = list.concat(list);
      const cur = S.lineDash;
      if (cur.length === list.length && cur.every((v, i) => v === list[i])) return;
      S.lineDash = list; opv(OP.lineDash, list);
    },
    getLineDash() { return S.lineDash.slice(); },

    fillGlass(a, b) {
      let path = null, o = a;
      if (a instanceof Path2D) { path = a; o = b; }
      if (o === undefined || o === null) o = {};
      else if (typeof o !== 'object') throw new TypeError('fillGlass: options must be an object like { style, tint, interactive, rule }');
      const style = o.style === undefined ? 'regular' : o.style;
      if (style !== 'regular' && style !== 'clear') throw new TypeError(`fillGlass: style must be 'regular' or 'clear', got ${describe(style)}`);
      const rule = ruleCode(o.rule, 'fillGlass');
      const tint = o.tint === undefined || o.tint === null ? -1 : str(String(o.tint));
      const args = [style === 'clear' ? 1 : 0, tint, o.interactive ? 1 : 0, rule, path ? 1 : 0];
      if (path !== null && !emitPath(path, args.length)) return;
      opv(OP.fillGlass, args);
    },
    fillBlur(a, b) {
      let path = null, o = a;
      if (a instanceof Path2D) { path = a; o = b; }
      if (o === undefined || o === null) o = {};
      else if (typeof o !== 'object') throw new TypeError('fillBlur: options must be an object like { material, rule }');
      const material = o.material === undefined ? 'hud' : o.material;
      const code = MATERIALS[material];
      if (code === undefined) throw new TypeError(`fillBlur: material must be one of ${Object.keys(MATERIALS).join(', ')}, got ${describe(material)}`);
      const rule = ruleCode(o.rule, 'fillBlur');
      if (path !== null && !emitPath(path, 3)) return;
      opv(OP.fillBlur, [code, rule, path ? 1 : 0]);
    },

    getImageData: unsupported('getImageData', 'drawings cannot read pixels'),
    putImageData: unsupported('putImageData'),
    createImageData: unsupported('createImageData'),
    createPattern: unsupported('createPattern', 'use gradients or draw the pattern with paths'),
    isPointInPath: unsupported('isPointInPath', 'do hit-testing yourself with math (input.mouse)'),
    isPointInStroke: unsupported('isPointInStroke', 'do hit-testing yourself with math (input.mouse)'),
    toDataURL: unsupported('toDataURL'),
  };
  // Four finite numbers → one op (canvas ignores calls with non-finite arguments).
  function rect4(code, a, b, c, d) {
    a = +a; b = +b; c = +c; d = +d;
    if (isF(a) && isF(b) && isF(c) && isF(d)) op4(code, a, b, c, d);
  }
  function text(code, t, x, y, w) {
    t = String(t);
    if (/[\t\n\f\r]/.test(t)) t = t.replace(/[\t\n\f\r]/g, ' ');
    x = +x; y = +y;
    if (!isF(x) || !isF(y)) return;
    let mw = NaN;
    if (w !== undefined) { mw = +w; if (!isF(mw) || mw <= 0) return; }
    op4(code, str(t), x, y, mw);
  }
  const accessors = {
    fillStyle: [() => S.fillStyle, v => setPaint('fill', v)],
    strokeStyle: [() => S.strokeStyle, v => setPaint('stroke', v)],
    lineWidth: [() => S.lineWidth, v => setNumber('lineWidth', OP.lineWidth, v, x => x > 0)],
    lineCap: [() => S.lineCap, v => setEnum('lineCap', OP.lineCap, CAPS, v)],
    lineJoin: [() => S.lineJoin, v => setEnum('lineJoin', OP.lineJoin, JOINS, v)],
    miterLimit: [() => S.miterLimit, v => setNumber('miterLimit', OP.miterLimit, v, x => x > 0)],
    lineDashOffset: [() => S.lineDashOffset, v => setNumber('lineDashOffset', OP.lineDashOffset, v, () => true)],
    globalAlpha: [() => S.globalAlpha, v => setNumber('globalAlpha', OP.globalAlpha, v, x => x >= 0 && x <= 1)],
    globalCompositeOperation: [() => S.globalCompositeOperation, v => setEnum('globalCompositeOperation', OP.globalCompositeOperation, COMPOSITES, v)],
    shadowColor: [() => S.shadowColor, v => { v = String(v); if (S.shadowColor !== v) { S.shadowColor = v; op1(OP.shadowColor, str(v)); } }],
    shadowBlur: [() => S.shadowBlur, v => setNumber('shadowBlur', OP.shadowBlur, v, x => x >= 0)],
    shadowOffsetX: [() => S.shadowOffsetX, v => setNumber('shadowOffsetX', OP.shadowOffsetX, v, () => true)],
    shadowOffsetY: [() => S.shadowOffsetY, v => setNumber('shadowOffsetY', OP.shadowOffsetY, v, () => true)],
    filter: [() => S.filter, v => {
      v = String(v);
      let r;
      if (v.trim() === 'none') r = 0;
      else {
        const m = /^\s*blur\(\s*(\d*\.?\d+)\s*(px)?\s*\)\s*$/.exec(v);
        if (m === null) { warnOnce('filter:' + v, `filter ${JSON.stringify(v)} is not supported; only 'blur(Npx)' and 'none' are`); return; }
        r = parseFloat(m[1]);
      }
      if (S.filter !== v) { S.filter = v; op1(OP.filterBlur, r); }
    }],
    font: [() => S.font, v => {
      v = String(v);
      if (!/(\d*\.?\d+)(px|pt)/.test(v)) { warnOnce('font:' + v, `font ${JSON.stringify(v)} has no size like '8px'; keeping ${JSON.stringify(S.font)}`); return; }
      if (S.font !== v) { S.font = v; op1(OP.font, str(v)); }
    }],
    textAlign: [() => S.textAlign, v => setEnum('textAlign', OP.textAlign, ALIGNS, v)],
    textBaseline: [() => S.textBaseline, v => setEnum('textBaseline', OP.textBaseline, BASELINES, v)],
    vibrant: [() => vibrant, v => { v = !!v; if (v !== vibrant) { vibrant = v; op1(OP.vibrant, v ? 1 : 0); } }],
  };
  for (const key of Object.keys(accessors)) {
    const [get, set] = accessors[key];
    Object.defineProperty(ctx, key, { get, set, enumerable: true, configurable: false });
  }

  // ---- peek -------------------------------------------------------------------
  let frameFn = null;
  const handlers = new Map();
  function format(v) {
    if (typeof v === 'string') return v;
    if (v instanceof Error) return `${v.name}: ${v.message}`;
    if (v !== null && typeof v === 'object') {
      try { const s = JSON.stringify(v); return s.length > 2000 ? s.slice(0, 2000) + '…' : s; } catch (e) { return String(v); }
    }
    return String(v);
  }
  const peek = Object.freeze({
    frame(fn) {
      if (typeof fn !== 'function') throw new TypeError(`peek.frame: expected a function, got ${describe(fn)}`);
      frameFn = fn;
    },
    on(name, fn) {
      if (typeof fn !== 'function') throw new TypeError(`peek.on: expected a function for '${name}', got ${describe(fn)}`);
      if (name === 'word') {
        warnOnce('word', "peek.on('word') never fires: peek has no word timing (use input.speech.progress and input.speech.level)");
        return;
      }
      if (!EVENTS.includes(name)) throw new TypeError(`peek.on: unknown event ${describe(name)}; events are ${EVENTS.join(', ')}`);
      let list = handlers.get(name);
      if (list === undefined) { list = []; handlers.set(name, list); }
      list.push(fn);
    },
    log(...args) {
      if (logCount >= logLimit) return;
      logCount++;
      LOG(logCount === logLimit ? '… (more peek.log lines were dropped)' : args.map(format).join(' '));
    },
  });
  const fixed = { writable: false, enumerable: false, configurable: false };
  Object.defineProperty(globalThis, 'peek', Object.assign({ value: peek }, fixed));
  Object.defineProperty(globalThis, 'Path2D', Object.assign({ value: Path2D }, fixed));
  Object.defineProperty(globalThis, 'CanvasGradient', Object.assign({ value: CanvasGradient }, fixed));

  globalThis.__peek_frame = function (input) {
    n = 0; opCount = 0; dropped = 0; full = false; strings = []; stringIds.clear();
    logCount = 0; logLimit = 20;
    S = defaults(); stack.length = 0; vibrant = false;
    inFrame = true;
    let again = false;
    try {
      if (frameFn !== null) {
        const r = frameFn(ctx, input);
        if (r !== null && typeof r === 'object' && typeof r.then === 'function') {
          warnOnce('async', 'peek.frame callbacks must be synchronous; a returned Promise counts as false');
        } else {
          again = !!r;
        }
      }
    } finally {
      inFrame = false;
    }
    OPS[n++] = OP.frameInfo; OPS[n++] = 3; OPS[n++] = opCount; OPS[n++] = dropped; OPS[n++] = frameFn !== null ? 1 : 0;
    FLUSH(n, strings, again);
  };

  globalThis.__peek_event = function (name, payload) {
    logCount = 0; logLimit = 20;
    const list = handlers.get(name);
    if (list === undefined) return;
    let error = null;
    for (const fn of list.slice()) {
      try { fn(payload); } catch (e) { if (error === null) error = e; }
    }
    if (error !== null) throw error;
  };
})();
"""#
    // swiftlint:enable line_length
}
