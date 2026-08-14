import { cpSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const desktopDirectory = dirname(scriptDirectory);
const iconDirectory = join(desktopDirectory, "src-tauri", "icons");
const svgPath = join(iconDirectory, "tendi-icon.svg");
const pngPath = join(iconDirectory, "icon.png");
const icnsPath = join(iconDirectory, "icon.icns");

// The reference image is translated into a small, editable shape model. Keep
// the geometry and palette here; generated SVG/PNG/ICNS files are outputs.
const icon = {
  canvas: 1024,
  tile: {
    x: 64,
    y: 64,
    size: 896,
    radius: 220,
  },
  topRibbon: {
    left: 236,
    top: 244,
    shoulderX: 397,
    plateauEndX: 698,
    tipX: 804,
    tipY: 211,
    neckX: 596,
    baseline: 448,
  },
  stem: {
    left: 397,
    top: 420,
    right: 590,
    bottom: 810,
    foldStartX: 431,
    foldStartY: 596,
  },
  palette: {
    tileTop: "#3C3836",
    tileMiddle: "#282828",
    tileBottom: "#1D2021",
    tileAccent: "#D65D0E",
    rimLight: "#A89984",
    rimDark: "#504945",
    silverWarm: "#FBF1C7",
    silverLight: "#EBDBB2",
    silverMid: "#D5C4A1",
    silverDark: "#7C6F64",
    foldLight: "#FE8019",
    foldMid: "#D65D0E",
    foldDark: "#AF3A03",
    edge: "#F9F5D7",
    flare: "#FABD2F",
  },
  effects: {
    tileShadowY: 28,
    tileShadowBlur: 32,
    markShadowY: 20,
    markShadowBlur: 22,
  },
};

function topRibbonPath({
  left,
  top,
  shoulderX,
  plateauEndX,
  tipX,
  tipY,
  neckX,
  baseline,
}) {
  return [
    `M ${left} ${baseline}`,
    `V ${top + 148}`,
    `C ${left} ${top + 64} ${left + 70} ${top} ${shoulderX} ${top}`,
    `H ${plateauEndX}`,
    `C ${plateauEndX + 48} ${top} ${tipX - 24} ${top - 7} ${tipX} ${tipY}`,
    `C ${tipX - 8} ${top + 79} ${tipX - 39} ${top + 134} ${tipX - 90} ${top + 174}`,
    `C ${tipX - 123} ${baseline - 9} ${tipX - 163} ${baseline} ${neckX} ${baseline}`,
    `H ${left}`,
    "Z",
  ].join(" ");
}

function rearFoldPath({ tipX, tipY, neckX, baseline }) {
  return [
    `M ${neckX} ${baseline}`,
    `C ${tipX - 158} ${baseline} ${tipX - 123} ${baseline - 9} ${tipX - 90} ${baseline - 30}`,
    `C ${tipX - 39} ${baseline - 70} ${tipX - 8} ${baseline - 125} ${tipX} ${tipY}`,
    `C ${tipX + 14} ${tipY + 126} ${tipX - 13} ${tipY + 238} ${tipX - 82} ${tipY + 296}`,
    `C ${tipX - 119} ${tipY + 328} ${tipX - 162} ${tipY + 342} ${neckX} ${tipY + 342}`,
    "Z",
  ].join(" ");
}

function stemPath({ left, top, right, bottom }) {
  return `M ${left} ${top} H ${right} V ${bottom} H ${left} Z`;
}

function stemFacePath({ right, bottom, foldStartX, foldStartY, top }) {
  return [
    `M ${foldStartX} ${top}`,
    `H ${right}`,
    `V ${bottom}`,
    `C ${right - 93} ${bottom - 10} ${foldStartX} ${bottom - 91} ${foldStartX} ${foldStartY}`,
    "Z",
  ].join(" ");
}

function createSvg(spec) {
  const { canvas, tile, topRibbon, stem, palette, effects } = spec;
  const tileBottom = tile.y + tile.size;
  const tileRight = tile.x + tile.size;

  return `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${canvas} ${canvas}" role="img" aria-labelledby="title description">
  <title id="title">Tendi app icon</title>
  <desc id="description">A folded cream and orange T ribbon on a warm charcoal tile.</desc>

  <defs>
    <linearGradient id="tile-fill" x1="190" y1="110" x2="850" y2="930" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="${palette.tileTop}"/>
      <stop offset="0.52" stop-color="${palette.tileMiddle}"/>
      <stop offset="1" stop-color="${palette.tileBottom}"/>
    </linearGradient>
    <radialGradient id="tile-violet" cx="0" cy="0" r="1" gradientTransform="translate(850 850) rotate(-132) scale(420 500)" gradientUnits="userSpaceOnUse">
      <stop stop-color="${palette.tileAccent}" stop-opacity="0.66"/>
      <stop offset="1" stop-color="${palette.tileAccent}" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="tile-rim" x1="182" y1="90" x2="864" y2="942" gradientUnits="userSpaceOnUse">
      <stop stop-color="${palette.rimLight}" stop-opacity="0.86"/>
      <stop offset="0.46" stop-color="${palette.rimDark}" stop-opacity="0.28"/>
      <stop offset="1" stop-color="${palette.tileAccent}" stop-opacity="0.95"/>
    </linearGradient>

    <linearGradient id="fold-fill" x1="620" y1="270" x2="750" y2="540" gradientUnits="userSpaceOnUse">
      <stop stop-color="${palette.silverDark}"/>
      <stop offset="0.48" stop-color="${palette.foldMid}"/>
      <stop offset="1" stop-color="${palette.foldDark}"/>
    </linearGradient>
    <linearGradient id="stem-fill" x1="392" y1="420" x2="600" y2="812" gradientUnits="userSpaceOnUse">
      <stop stop-color="${palette.silverDark}"/>
      <stop offset="0.22" stop-color="${palette.silverMid}"/>
      <stop offset="0.68" stop-color="${palette.silverDark}"/>
      <stop offset="1" stop-color="${palette.foldLight}"/>
    </linearGradient>
    <linearGradient id="stem-face" x1="435" y1="420" x2="570" y2="808" gradientUnits="userSpaceOnUse">
      <stop stop-color="${palette.silverLight}" stop-opacity="0.42"/>
      <stop offset="0.46" stop-color="${palette.silverMid}" stop-opacity="0.2"/>
      <stop offset="1" stop-color="${palette.foldLight}" stop-opacity="0.78"/>
    </linearGradient>
    <linearGradient id="ribbon-fill" x1="236" y1="312" x2="804" y2="370" gradientUnits="userSpaceOnUse">
      <stop stop-color="${palette.silverWarm}"/>
      <stop offset="0.28" stop-color="${palette.silverLight}"/>
      <stop offset="0.66" stop-color="${palette.silverMid}"/>
      <stop offset="1" stop-color="${palette.silverLight}"/>
    </linearGradient>
    <radialGradient id="ribbon-sheen" cx="0" cy="0" r="1" gradientTransform="translate(679 267) rotate(149) scale(332 196)" gradientUnits="userSpaceOnUse">
      <stop stop-color="${palette.silverWarm}" stop-opacity="0.8"/>
      <stop offset="0.55" stop-color="${palette.silverWarm}" stop-opacity="0.08"/>
      <stop offset="1" stop-color="${palette.silverWarm}" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="intersection-flare" x1="425" y1="445" x2="425" y2="592" gradientUnits="userSpaceOnUse">
      <stop stop-color="${palette.flare}" stop-opacity="0.96"/>
      <stop offset="0.55" stop-color="${palette.flare}" stop-opacity="0.34"/>
      <stop offset="1" stop-color="${palette.flare}" stop-opacity="0"/>
    </linearGradient>

    <filter id="tile-shadow" x="-20%" y="-20%" width="140%" height="150%" color-interpolation-filters="sRGB">
      <feDropShadow dx="0" dy="${effects.tileShadowY}" stdDeviation="${effects.tileShadowBlur}" flood-color="${palette.tileBottom}" flood-opacity="0.38"/>
    </filter>
    <filter id="mark-shadow" x="-25%" y="-25%" width="160%" height="170%" color-interpolation-filters="sRGB">
      <feDropShadow dx="7" dy="${effects.markShadowY}" stdDeviation="${effects.markShadowBlur}" flood-color="${palette.tileBottom}" flood-opacity="0.72"/>
    </filter>
    <filter id="flare-blur" x="392" y="424" width="64" height="183" filterUnits="userSpaceOnUse">
      <feGaussianBlur stdDeviation="9"/>
    </filter>
    <clipPath id="tile-clip">
      <rect x="${tile.x}" y="${tile.y}" width="${tile.size}" height="${tile.size}" rx="${tile.radius}"/>
    </clipPath>
    <clipPath id="ribbon-clip">
      <path d="${topRibbonPath(topRibbon)}"/>
    </clipPath>
  </defs>

  <g id="tile" filter="url(#tile-shadow)">
    <rect x="${tile.x}" y="${tile.y}" width="${tile.size}" height="${tile.size}" rx="${tile.radius}" fill="url(#tile-fill)"/>
    <rect x="${tile.x}" y="${tile.y}" width="${tile.size}" height="${tile.size}" rx="${tile.radius}" fill="url(#tile-violet)"/>
    <path d="M ${tile.x + 30} ${tile.y + 235} C ${tile.x + 47} ${tile.y + 105} ${tile.x + 118} ${tile.y + 35} ${tile.x + 255} ${tile.y + 24} H ${tileRight - 235}" fill="none" stroke="${palette.silverWarm}" stroke-opacity="0.1" stroke-width="13" stroke-linecap="round" clip-path="url(#tile-clip)"/>
    <rect x="${tile.x + 7}" y="${tile.y + 7}" width="${tile.size - 14}" height="${tile.size - 14}" rx="${tile.radius - 7}" fill="none" stroke="url(#tile-rim)" stroke-width="13"/>
    <rect x="${tile.x + 22}" y="${tile.y + 22}" width="${tile.size - 44}" height="${tile.size - 44}" rx="${tile.radius - 22}" fill="none" stroke="${palette.tileBottom}" stroke-opacity="0.48" stroke-width="5"/>
    <path d="M ${tile.x + 190} ${tileBottom - 22} H ${tileRight - 205} C ${tileRight - 114} ${tileBottom - 22} ${tileRight - 49} ${tileBottom - 74} ${tileRight - 28} ${tileBottom - 163}" fill="none" stroke="${palette.tileAccent}" stroke-opacity="0.46" stroke-width="11" stroke-linecap="round"/>
  </g>

  <g id="folded-t" filter="url(#mark-shadow)">
    <path id="rear-fold" d="${rearFoldPath(topRibbon)}" fill="url(#fold-fill)" stroke="${palette.foldLight}" stroke-opacity="0.68" stroke-width="7" stroke-linejoin="round"/>

    <path id="stem" d="${stemPath(stem)}" fill="url(#stem-fill)" stroke="${palette.edge}" stroke-width="7" stroke-linejoin="round"/>
    <path id="stem-face-shape" d="${stemFacePath(stem)}" fill="url(#stem-face)"/>
    <path d="M ${stem.foldStartX} ${stem.top + 19} V ${stem.foldStartY} C ${stem.foldStartX} ${stem.bottom - 90} ${stem.right - 93} ${stem.bottom - 10} ${stem.right - 4} ${stem.bottom - 4}" fill="none" stroke="${palette.edge}" stroke-opacity="0.78" stroke-width="6" stroke-linecap="round"/>
    <path d="M ${stem.left + 10} ${stem.top + 12} V ${stem.bottom - 12}" fill="none" stroke="${palette.silverWarm}" stroke-opacity="0.24" stroke-width="5" stroke-linecap="round"/>

    <path id="top-ribbon" d="${topRibbonPath(topRibbon)}" fill="url(#ribbon-fill)" stroke="${palette.edge}" stroke-width="7" stroke-linejoin="round"/>
    <ellipse cx="610" cy="275" rx="330" ry="185" fill="url(#ribbon-sheen)" clip-path="url(#ribbon-clip)"/>
    <path d="M ${topRibbon.left + 14} ${topRibbon.top + 143} C ${topRibbon.left + 17} ${topRibbon.top + 72} ${topRibbon.left + 77} ${topRibbon.top + 18} ${topRibbon.shoulderX + 2} ${topRibbon.top + 18} H ${topRibbon.plateauEndX - 7} C ${topRibbon.plateauEndX + 37} ${topRibbon.top + 18} ${topRibbon.tipX - 25} ${topRibbon.top + 8} ${topRibbon.tipX - 8} ${topRibbon.tipY + 10}" fill="none" stroke="${palette.silverWarm}" stroke-opacity="0.72" stroke-width="7" stroke-linecap="round"/>
    <path d="M ${topRibbon.left + 5} ${topRibbon.baseline - 4} H ${topRibbon.neckX} C ${topRibbon.tipX - 158} ${topRibbon.baseline - 4} ${topRibbon.tipX - 120} ${topRibbon.baseline - 16} ${topRibbon.tipX - 88} ${topRibbon.baseline - 39}" fill="none" stroke="${palette.rimDark}" stroke-opacity="0.86" stroke-width="9" stroke-linecap="round"/>

    <path d="M ${stem.foldStartX - 5} ${stem.top + 31} V ${stem.top + 173}" fill="none" stroke="url(#intersection-flare)" stroke-width="8"/>
    <path d="M ${stem.foldStartX - 7} ${stem.top + 36} V ${stem.top + 155}" fill="none" stroke="url(#intersection-flare)" stroke-width="21" stroke-linecap="round" filter="url(#flare-blur)" opacity="0.7"/>
  </g>
</svg>
`;
}

function run(command, args, cwd = desktopDirectory) {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status}`);
  }
}

writeFileSync(svgPath, createSvg(icon));

if (process.platform !== "darwin") {
  throw new Error("PNG and ICNS generation currently requires macOS sips and icon tooling.");
}

run("sips", [
  "-s",
  "format",
  "png",
  "--resampleHeightWidth",
  String(icon.canvas),
  String(icon.canvas),
  svgPath,
  "--out",
  pngPath,
]);

const generatedDirectory = mkdtempSync(join(tmpdir(), "tendi-app-icon-"));
try {
  run("npm", [
    "exec",
    "--",
    "tauri",
    "icon",
    svgPath,
    "--output",
    generatedDirectory,
  ]);
  cpSync(join(generatedDirectory, "icon.icns"), icnsPath);
} finally {
  rmSync(generatedDirectory, { recursive: true, force: true });
}

console.log(`Generated ${svgPath}`);
console.log(`Generated ${pngPath}`);
console.log(`Generated ${icnsPath}`);
