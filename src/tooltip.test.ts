import { describe, expect, it } from "vitest";
import { calculateTooltipPosition } from "./tooltip";

const anchor = (overrides: Partial<DOMRect> = {}): DOMRect =>
  ({
    left: 100,
    top: 100,
    right: 118,
    bottom: 118,
    width: 18,
    height: 18,
    x: 100,
    y: 100,
    toJSON: () => ({}),
    ...overrides,
  }) as DOMRect;

describe("calculateTooltipPosition", () => {
  it("prefers the space above the trigger", () => {
    expect(
      calculateTooltipPosition(anchor(), { width: 160, height: 48 }, { innerWidth: 400, innerHeight: 300 }),
    ).toEqual({ left: 29, top: 44, placement: "above" });
  });

  it("uses the space below when above would cross the viewport gutter", () => {
    expect(
      calculateTooltipPosition(
        anchor({ top: 12, bottom: 30 }),
        { width: 160, height: 48 },
        { innerWidth: 400, innerHeight: 300 },
      ),
    ).toEqual({ left: 29, top: 38, placement: "below" });
  });

  it("clamps a wide tooltip inside the horizontal viewport gutter", () => {
    expect(
      calculateTooltipPosition(
        anchor({ left: 380, right: 398 }),
        { width: 160, height: 48 },
        { innerWidth: 400, innerHeight: 300 },
      ),
    ).toMatchObject({ left: 232, placement: "above" });
  });
});
