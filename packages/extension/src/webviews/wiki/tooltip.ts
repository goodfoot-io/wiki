/// <reference lib="dom" />
/**
 * Floating tooltip for file-link hover previews.
 *
 * Creates a single fixed-position `#wiki-tooltip` element and repositions it
 * on each show call. Styled via `media/tooltip.css` using VSCode hover-widget
 * CSS variables for automatic light/dark support.
 *
 * @summary Floating tooltip DOM management for file-link hover previews.
 */

import '@vscode-elements/elements/dist/vscode-badge/index.js';

let tooltipEl: HTMLDivElement | null = null;
let bodyEl: HTMLDivElement | null = null;
let arrowEl: HTMLDivElement | null = null;

const SIDE_OFFSET = 6;
const ARROW_HALF = 4;
const EDGE_MARGIN = 8;

/** Create and append the tooltip element to the document body. Call once at startup. */
export function initTooltip(): void {
  tooltipEl = document.createElement('div');
  tooltipEl.id = 'wiki-tooltip';
  tooltipEl.setAttribute('role', 'tooltip');

  bodyEl = document.createElement('div');
  bodyEl.className = 'wiki-tooltip-body';
  tooltipEl.appendChild(bodyEl);

  arrowEl = document.createElement('div');
  arrowEl.className = 'wiki-tooltip-arrow';
  tooltipEl.appendChild(arrowEl);

  document.body.appendChild(tooltipEl);
}

const FILE_LANG: Record<string, string> = {
  ts: 'TypeScript',
  tsx: 'TypeScript',
  js: 'JavaScript',
  jsx: 'JavaScript',
  rs: 'Rust',
  go: 'Go',
  py: 'Python',
  rb: 'Ruby',
  java: 'Java',
  c: 'C',
  cpp: 'C++',
  cs: 'C#',
  md: 'Markdown',
  json: 'JSON',
  yaml: 'YAML',
  toml: 'TOML'
};

/**
 * Show a tooltip for a markdown link href that points at a file.
 *
 * The href may include a line-range fragment (e.g. `path/to/file.ts#L10-L45`)
 * or a `@sha` git pin produced by `wiki check --fix`.
 *
 * @param anchor - The hovered anchor element.
 * @param href - The raw href attribute of the anchor.
 */
export function showFileTooltip(anchor: HTMLElement, href: string): void {
  if (bodyEl == null) return;

  const hashIdx = href.indexOf('#');
  const rawPath = hashIdx >= 0 ? href.slice(0, hashIdx) : href;
  const fragment = hashIdx >= 0 ? href.slice(hashIdx + 1) : '';

  const atIdx = rawPath.indexOf('@');
  const filePath = atIdx >= 0 ? rawPath.slice(0, atIdx) : rawPath;

  const ext = filePath.split('.').pop()?.toLowerCase() ?? '';
  const lang = FILE_LANG[ext];

  let lineHtml = '';
  const lineMatch = fragment.match(/^L(\d+)(?:-L?(\d+))?$/i);
  if (lineMatch) {
    const [, start, end] = lineMatch;
    lineHtml = `<div class="wiki-tooltip-summary">${end ? `Lines ${start}–${end}` : `Line ${start}`}</div>`;
  }

  const badgeHtml =
    lang != null ? `<div class="wiki-tooltip-tags"><vscode-badge>${escapeHtml(lang)}</vscode-badge></div>` : '';
  bodyEl.innerHTML = `<div class="wiki-tooltip-title">${escapeHtml(filePath)}</div>${lineHtml}${badgeHtml}`;
  positionAndShow(anchor);
}

function positionAndShow(anchor: HTMLElement): void {
  if (tooltipEl == null || arrowEl == null) return;

  tooltipEl.style.visibility = 'hidden';
  tooltipEl.classList.add('wiki-tooltip--visible');

  const rect = anchor.getBoundingClientRect();
  const tipWidth = tooltipEl.offsetWidth;
  const tipHeight = tooltipEl.offsetHeight;

  const showAbove = rect.top > tipHeight + SIDE_OFFSET + EDGE_MARGIN;
  const top = showAbove ? rect.top - tipHeight - SIDE_OFFSET : rect.bottom + SIDE_OFFSET;
  arrowEl.className = `wiki-tooltip-arrow wiki-tooltip-arrow--${showAbove ? 'down' : 'up'}`;

  const anchorCenterX = rect.left + rect.width / 2;
  const left = Math.max(
    EDGE_MARGIN,
    Math.min(anchorCenterX - tipWidth / 2, window.innerWidth - tipWidth - EDGE_MARGIN)
  );

  arrowEl.style.left = `${Math.max(ARROW_HALF + 2, Math.min(anchorCenterX - left, tipWidth - ARROW_HALF - 2)) - ARROW_HALF}px`;

  tooltipEl.style.top = `${top}px`;
  tooltipEl.style.left = `${left}px`;
  tooltipEl.style.visibility = '';
}

/** Remove the visible class, triggering the CSS fade-out. */
export function hideTooltip(): void {
  tooltipEl?.classList.remove('wiki-tooltip--visible');
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
