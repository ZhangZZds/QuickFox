export type TooltipPosition = {
  left: number;
  top: number;
  placement: "above" | "below";
};

const GUTTER = 8;
const GAP = 8;

export function calculateTooltipPosition(
  anchor: DOMRect,
  tooltip: Pick<DOMRect, "width" | "height">,
  viewport: Pick<Window, "innerWidth" | "innerHeight">,
): TooltipPosition {
  const spaceAbove = anchor.top - GAP - GUTTER;
  const spaceBelow = viewport.innerHeight - anchor.bottom - GAP - GUTTER;
  const placement = spaceAbove >= tooltip.height || spaceAbove >= spaceBelow ? "above" : "below";
  const top =
    placement === "above"
      ? Math.max(GUTTER, anchor.top - GAP - tooltip.height)
      : Math.min(viewport.innerHeight - GUTTER - tooltip.height, anchor.bottom + GAP);
  const left = Math.max(
    GUTTER,
    Math.min(
      anchor.left + anchor.width / 2 - tooltip.width / 2,
      viewport.innerWidth - GUTTER - tooltip.width,
    ),
  );

  return { left, top, placement };
}
