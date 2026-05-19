/// <reference lib="dom" />
/**
 * Floating tooltip for file-link hover previews.
 *
 * Creates a single fixed-position `#wiki-tooltip` element and repositions it
 * on each show call. The tooltip is ALWAYS placed above the hovered link and
 * re-oriented horizontally at viewport edges. Styled via `media/tooltip.css`
 * using VSCode hover-widget CSS variables for automatic light/dark support.
 *
 * @summary Floating tooltip DOM management for file-link hover previews.
 */

let tooltipEl: HTMLDivElement | null = null;
let bodyEl: HTMLDivElement | null = null;
let arrowEl: HTMLDivElement | null = null;

const SIDE_OFFSET = 6;
const ARROW_HALF = 4;
const EDGE_MARGIN = 8;

/** Minimal rectangle of the hovered anchor in viewport coordinates. */
export interface AnchorRect {
  top: number;
  left: number;
  width: number;
  height: number;
  bottom: number;
}

/** Measured size of the tooltip box (or the viewport). */
export interface Size {
  width: number;
  height: number;
}

/** Resolved tooltip placement in viewport coordinates. */
export interface TooltipPlacement {
  /** Box top edge (px from viewport top). */
  top: number;
  /** Box left edge (px from viewport left). */
  left: number;
  /** Arrow orientation — `down` when the box is above the anchor. */
  arrow: 'down' | 'up';
  /** Arrow left offset within the box (px from the box's left edge). */
  arrowLeft: number;
}

/** Inputs to {@link buildTooltipHtml}. */
export interface TooltipContent {
  /** Repo-root-relative path of the link target. */
  relPath: string;
  /** Wiki page title (present only when the target is a wiki page). */
  title?: string;
  /** Wiki page summary (present only when the target is a wiki page). */
  summary?: string;
}

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

/**
 * Compute the always-above, edge-aware placement for the tooltip box.
 *
 * The box is positioned entirely above the anchor (its bottom never reaches
 * the anchor's top), with the down-pointing arrow aimed at the anchor's
 * horizontal center. The box is clamped within the viewport by
 * `EDGE_MARGIN`; near an edge the box shifts inward while the arrow tracks
 * the anchor (toward the box's right side near the right edge, toward its
 * left side near the left edge), and is clamped to remain on the box.
 *
 * @param anchor   - The hovered anchor's bounding rectangle.
 * @param tip      - The measured tooltip box size.
 * @param viewport - The available viewport size.
 * @returns The resolved {@link TooltipPlacement}.
 */
export function computeTooltipPlacement(anchor: AnchorRect, tip: Size, viewport: Size): TooltipPlacement {
  const top = anchor.top - tip.height - SIDE_OFFSET;

  const anchorCenterX = anchor.left + anchor.width / 2;
  const left = Math.max(EDGE_MARGIN, Math.min(anchorCenterX - tip.width / 2, viewport.width - tip.width - EDGE_MARGIN));

  const arrowCenter = Math.max(ARROW_HALF + 2, Math.min(anchorCenterX - left, tip.width - ARROW_HALF - 2));

  return { top, left, arrow: 'down', arrowLeft: arrowCenter - ARROW_HALF };
}

/**
 * Build the inner HTML for a file-link tooltip.
 *
 * A wiki page (both `title` and `summary` present) renders the bold title,
 * the repo-root-relative path in gray italics, then the summary as prose.
 * Any other file renders only the repo-root-relative path in italics. No
 * badge or line-range is ever rendered.
 *
 * @param content - The resolved link-target info.
 * @returns Inner HTML string for the tooltip body.
 */
export function buildTooltipHtml(content: TooltipContent): string {
  const pathHtml = `<div class="wiki-tooltip-path">${escapeHtml(content.relPath)}</div>`;
  if (content.title != null && content.title.length > 0 && content.summary != null && content.summary.length > 0) {
    return (
      `<div class="wiki-tooltip-title">${escapeHtml(content.title)}</div>` +
      pathHtml +
      `<div class="wiki-tooltip-summary">${escapeHtml(content.summary)}</div>`
    );
  }
  return pathHtml;
}

/**
 * Show a tooltip for a resolved file link.
 *
 * @param anchor  - The hovered anchor element.
 * @param content - Resolved repo-root path plus optional wiki title/summary.
 */
export function showFileTooltip(anchor: HTMLElement, content: TooltipContent): void {
  if (bodyEl == null) return;
  bodyEl.innerHTML = buildTooltipHtml(content);
  positionAndShow(anchor);
}

function positionAndShow(anchor: HTMLElement): void {
  if (tooltipEl == null || arrowEl == null) return;

  tooltipEl.style.visibility = 'hidden';
  tooltipEl.classList.add('wiki-tooltip--visible');

  const rect = anchor.getBoundingClientRect();
  const placement = computeTooltipPlacement(
    { top: rect.top, left: rect.left, width: rect.width, height: rect.height, bottom: rect.bottom },
    { width: tooltipEl.offsetWidth, height: tooltipEl.offsetHeight },
    { width: window.innerWidth, height: window.innerHeight }
  );

  arrowEl.className = `wiki-tooltip-arrow wiki-tooltip-arrow--${placement.arrow}`;
  arrowEl.style.left = `${placement.arrowLeft}px`;

  tooltipEl.style.top = `${placement.top}px`;
  tooltipEl.style.left = `${placement.left}px`;
  tooltipEl.style.visibility = '';
}

/** Remove the visible class, triggering the CSS fade-out. */
export function hideTooltip(): void {
  tooltipEl?.classList.remove('wiki-tooltip--visible');
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
