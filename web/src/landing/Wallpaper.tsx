import { For } from "solid-js";

// An original, illustrated coastal wallpaper (dusk in light mode, a moonlit night in dark mode).
// Colours come from CSS custom properties (see styles/landing.css), so the theme switches without
// JavaScript. It exists to give the glass bubbles real detail to blur and refract.

const REFLECTION = Array.from({ length: 16 }, (_, i) => ({
  cy: 714 + i * 17,
  rx: 74 - i * 3.6,
  dx: (i % 3) * 6 - 6,
  opacity: 0.6 - i * 0.032,
}));

// Deterministic star field (no Math.random, so every render and screenshot matches).
const STARS = Array.from({ length: 70 }, (_, i) => {
  const x = (i * 227 + 41) % 1600;
  const y = (i * 131 + 17) % 560;
  return { x, y, r: i % 7 === 0 ? 1.8 : i % 3 === 0 ? 1.2 : 0.8, o: 0.35 + ((i * 37) % 60) / 100 };
});

const stop = (offset: string, color: string, opacity?: number) => (
  <stop offset={offset} style={{ "stop-color": color, "stop-opacity": opacity ?? 1 }} />
);

export default function Wallpaper() {
  return (
    <svg class="wallpaper" viewBox="0 0 1600 1000" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
      <defs>
        <linearGradient id="wp-sky" x1="0" y1="0" x2="0" y2="1">
          {stop("0", "var(--wp-sky-0)")}
          {stop(".38", "var(--wp-sky-1)")}
          {stop(".62", "var(--wp-sky-2)")}
          {stop(".7", "var(--wp-sky-3)")}
        </linearGradient>
        <radialGradient id="wp-glow" cx=".5" cy=".5" r=".5">
          {stop("0", "var(--wp-glow)", 0.95)}
          {stop(".45", "var(--wp-glow)", 0.35)}
          {stop("1", "var(--wp-glow)", 0)}
        </radialGradient>
        <linearGradient id="wp-sea" x1="0" y1="0" x2="0" y2="1">
          {stop("0", "var(--wp-sea-0)")}
          {stop(".45", "var(--wp-sea-1)")}
          {stop("1", "var(--wp-sea-2)")}
        </linearGradient>
        <linearGradient id="wp-sand" x1="0" y1="0" x2="0" y2="1">
          {stop("0", "var(--wp-sand-0)")}
          {stop("1", "var(--wp-sand-1)")}
        </linearGradient>
        <linearGradient id="wp-rock" x1="0" y1="0" x2="1" y2="1">
          {stop("0", "var(--wp-rock-lit)")}
          {stop(".55", "var(--wp-rock)")}
          {stop("1", "var(--wp-rock-dark)")}
        </linearGradient>
        <filter id="wp-soft" x="-20%" y="-50%" width="140%" height="200%">
          <feGaussianBlur stdDeviation="14" />
        </filter>
      </defs>

      <rect width="1600" height="1000" fill="url(#wp-sky)" />

      <g class="wp-night">
        <For each={STARS}>{(s) => <circle cx={s.x} cy={s.y} r={s.r} style={{ fill: "#fff", "fill-opacity": s.o }} />}</For>
      </g>

      <circle cx="1060" cy="610" r="430" fill="url(#wp-glow)" />
      <circle class="wp-day" cx="1060" cy="628" r="62" style={{ fill: "var(--wp-sun)" }} />
      <g class="wp-night">
        <circle cx="1402" cy="312" r="120" fill="url(#wp-glow)" style={{ opacity: 0.5 }} />
        <circle cx="1402" cy="312" r="34" style={{ fill: "var(--wp-sun)" }} />
        <circle cx="1415" cy="304" r="30" style={{ fill: "var(--wp-sky-1)", "fill-opacity": 0.93 }} />
      </g>

      <g filter="url(#wp-soft)" style={{ fill: "var(--wp-cloud)" }}>
        <ellipse cx="330" cy="240" rx="280" ry="24" />
        <ellipse cx="560" cy="300" rx="190" ry="14" />
        <ellipse cx="1280" cy="190" rx="220" ry="18" />
        <ellipse cx="1450" cy="330" rx="160" ry="12" />
      </g>

      <g class="wp-day" style={{ fill: "none", stroke: "var(--wp-bird)", "stroke-width": 2.4, "stroke-linecap": "round" }}>
        <path d="M402 262 q9 -7 18 0 q9 -7 18 0" />
        <path d="M452 238 q6 -5 12 0 q6 -5 12 0" />
        <path d="M1188 318 q7 -6 14 0 q7 -6 14 0" />
      </g>

      {/* distant hills and headlands */}
      <path
        d="M0 648 C120 610 230 566 340 588 C430 606 480 566 570 552 C660 538 730 580 810 596 C890 610 960 606 1010 614 L1600 646 L1600 704 L0 704 Z"
        style={{ fill: "var(--wp-hill-far)" }}
      />
      <path
        d="M0 596 C92 556 176 514 266 536 C338 552 392 606 482 638 C572 668 654 680 736 692 L736 706 L0 706 Z"
        style={{ fill: "var(--wp-hill-near)" }}
      />
      <path d="M1600 506 C1518 528 1466 596 1398 638 C1338 674 1286 690 1236 702 L1600 706 Z" style={{ fill: "var(--wp-cliff)" }} />
      <path d="M1600 560 C1560 574 1540 612 1500 640 C1470 662 1440 676 1410 690" style={{ fill: "none", stroke: "var(--wp-cliff-edge)", "stroke-width": 3 }} />

      {/* sea */}
      <rect y="700" width="1600" height="300" fill="url(#wp-sea)" />
      <rect y="699" width="1600" height="3" style={{ fill: "var(--wp-horizon)", "fill-opacity": 0.45 }} />
      <g style={{ fill: "var(--wp-sparkle)" }}>
        <For each={REFLECTION}>
          {(r) => <ellipse cx={1060 + r.dx} cy={r.cy} rx={r.rx} ry="2.2" style={{ "fill-opacity": r.opacity }} />}
        </For>
      </g>
      <g class="wp-night" style={{ fill: "var(--wp-sparkle)" }}>
        <For each={REFLECTION.slice(0, 10)}>
          {(r) => <ellipse cx={1392 + r.dx} cy={r.cy} rx={r.rx * 0.4} ry="1.8" style={{ "fill-opacity": r.opacity * 0.7 }} />}
        </For>
      </g>
      <g style={{ fill: "none", stroke: "var(--wp-foam)", "stroke-linecap": "round" }}>
        <path d="M0 842 C190 826 372 862 590 842 S990 824 1190 848 S1470 836 1600 842" style={{ "stroke-width": 2.5, "stroke-opacity": 0.45 }} />
        <path d="M0 904 C220 884 410 928 640 906 S1040 884 1260 910" style={{ "stroke-width": 3.5, "stroke-opacity": 0.55 }} />
        <path d="M90 958 C300 936 470 978 700 958" style={{ "stroke-width": 4, "stroke-opacity": 0.5 }} />
      </g>

      {/* beach and rocks */}
      <path d="M812 1000 C972 928 1176 888 1378 878 C1478 874 1560 878 1600 882 L1600 1000 Z" fill="url(#wp-sand)" />
      <path d="M1040 944 C1160 910 1300 896 1420 900" style={{ fill: "none", stroke: "var(--wp-foam)", "stroke-width": 3, "stroke-opacity": 0.6 }} />
      <g fill="url(#wp-rock)">
        <path d="M626 1000 C636 928 690 876 766 866 C842 856 906 896 936 948 C952 974 958 990 960 1000 Z" />
        <path d="M0 1000 L0 852 C44 830 118 842 160 874 C202 906 222 958 228 1000 Z" />
        <path d="M268 820 C280 794 326 782 360 792 C388 800 400 818 398 830 L268 832 Z" />
        <path d="M470 770 C480 756 506 750 526 756 C542 762 548 772 546 778 L470 780 Z" />
        <path d="M1284 1000 C1296 948 1350 916 1414 920 C1480 924 1522 966 1534 1000 Z" />
        <path d="M1480 880 C1490 860 1520 852 1548 858 C1570 864 1582 878 1580 888 L1480 890 Z" />
        <path d="M1096 862 C1104 848 1126 842 1144 846 C1158 850 1164 860 1162 866 L1096 868 Z" />
      </g>
      <g style={{ fill: "none", stroke: "var(--wp-rock-edge)", "stroke-width": 2, "stroke-linecap": "round" }}>
        <path d="M676 900 C700 880 736 870 770 872" />
        <path d="M30 858 C62 846 100 850 128 866" />
        <path d="M1310 948 C1330 930 1360 922 1392 924" />
      </g>
    </svg>
  );
}
