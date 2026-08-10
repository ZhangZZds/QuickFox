# Tooltip Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render every QuickFox settings help tooltip as a window-level overlay that remains readable while its settings content scrolls or reaches a window edge.

**Architecture:** Add a small pure geometry module for viewport-safe placement. `HelpIcon` owns hover/focus state and renders a `role="tooltip"` through a React portal; when open, it recalculates from the trigger and tooltip rectangles on capture-phase scrolling and window resize.

**Tech Stack:** React 19, TypeScript, React DOM portal, Vitest, Testing Library, CSS.

---

### Task 1: Viewport-safe placement geometry

**Files:**
- Create: `src/tooltip.ts`
- Create: `src/tooltip.test.ts`

- [ ] **Step 1: Write the failing geometry tests**

```ts
import { describe, expect, it } from "vitest";
import { calculateTooltipPosition } from "./tooltip";

const anchor = (overrides: Partial<DOMRect> = {}): DOMRect =>
  ({ left: 100, top: 100, right: 118, bottom: 118, width: 18, height: 18, x: 100, y: 100, toJSON: () => ({ }), ...overrides }) as DOMRect;

describe("calculateTooltipPosition", () => {
  it("prefers the space above the trigger", () => {
    expect(calculateTooltipPosition(anchor(), { width: 160, height: 48 }, { width: 400, height: 300 })).toEqual({ left: 29, top: 44, placement: "above" });
  });

  it("uses the space below when above would cross the viewport gutter", () => {
    expect(calculateTooltipPosition(anchor({ top: 12, bottom: 30 }), { width: 160, height: 48 }, { width: 400, height: 300 })).toEqual({ left: 29, top: 38, placement: "below" });
  });

  it("clamps a wide tooltip inside the horizontal viewport gutter", () => {
    expect(calculateTooltipPosition(anchor({ left: 380, right: 398 }), { width: 160, height: 48 }, { width: 400, height: 300 })).toMatchObject({ left: 232, placement: "above" });
  });
});
```

- [ ] **Step 2: Run the test to verify it fails because the module is missing**

Run: `npx vitest run src/tooltip.test.ts`

Expected: FAIL with a module-not-found error for `./tooltip`.

- [ ] **Step 3: Add the minimal pure placement module**

```ts
export type TooltipPosition = { left: number; top: number; placement: "above" | "below" };

const GUTTER = 8;
const GAP = 8;

export function calculateTooltipPosition(anchor: DOMRect, tooltip: Pick<DOMRect, "width" | "height">, viewport: Pick<Window, "innerWidth" | "innerHeight">): TooltipPosition {
  const spaceAbove = anchor.top - GAP - GUTTER;
  const spaceBelow = viewport.innerHeight - anchor.bottom - GAP - GUTTER;
  const placement = spaceAbove >= tooltip.height || spaceAbove >= spaceBelow ? "above" : "below";
  const top = placement === "above"
    ? Math.max(GUTTER, anchor.top - GAP - tooltip.height)
    : Math.min(viewport.innerHeight - GUTTER - tooltip.height, anchor.bottom + GAP);
  const left = Math.max(GUTTER, Math.min(anchor.left + anchor.width / 2 - tooltip.width / 2, viewport.innerWidth - GUTTER - tooltip.width));
  return { left, top, placement };
}
```

- [ ] **Step 4: Run the geometry tests to verify they pass**

Run: `npx vitest run src/tooltip.test.ts`

Expected: PASS with 3 tests.

- [ ] **Step 5: Commit the geometry behavior**

```bash
git add src/tooltip.ts src/tooltip.test.ts
git commit -m "feat: position help tooltips inside viewport"
```

### Task 2: Portal-based help tooltip

**Files:**
- Modify: `src/App.tsx:1-30,322-350`
- Modify: `src/App.test.tsx:1668-1700`
- Modify: `src/styles.css:733-788`

- [ ] **Step 1: Write the failing component regression test**

```tsx
it("renders focused help as a window-level tooltip and hides it on blur", async () => {
  render(<App initialView="settings" />);
  await screen.findByDisplayValue("/tmp");
  const trigger = screen.getByRole("button", { name: "索引目录说明" });
  fireEvent.focus(trigger);
  const tooltip = await screen.findByRole("tooltip");
  expect(tooltip).toHaveTextContent("每行填写一个完整目录路径");
  expect(tooltip.parentElement).toBe(document.body);
  expect(trigger).toHaveAttribute("aria-describedby", tooltip.id);
  fireEvent.blur(trigger);
  expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
});
```

- [ ] **Step 2: Run the focused test to verify the old DOM-local tooltip fails the window-level expectation**

Run: `npx vitest run src/App.test.tsx -t "window-level tooltip"`

Expected: FAIL because the tooltip is not a direct child of `document.body` and remains always rendered.

- [ ] **Step 3: Render HelpIcon through a portal and recompute while visible**

```tsx
const tooltipId = useId();
const [visible, setVisible] = useState(false);
const triggerRef = useRef<HTMLButtonElement>(null);
const tooltipRef = useRef<HTMLSpanElement>(null);
const updatePosition = useCallback(() => {
  if (triggerRef.current && tooltipRef.current) {
    setPosition(calculateTooltipPosition(triggerRef.current.getBoundingClientRect(), tooltipRef.current.getBoundingClientRect(), window));
  }
}, []);

useLayoutEffect(() => {
  if (!visible) return;
  updatePosition();
  window.addEventListener("resize", updatePosition);
  window.addEventListener("scroll", updatePosition, true);
  return () => { window.removeEventListener("resize", updatePosition); window.removeEventListener("scroll", updatePosition, true); };
}, [visible, updatePosition]);
```

Render the tooltip with `createPortal(...)` only while visible. Bind `onPointerEnter`, `onPointerLeave`, `onFocus`, and `onBlur` on the button; use a fixed-position style populated by `calculateTooltipPosition`. Make `FullPathValue` reuse `HelpIcon` rather than carry a second DOM-local tooltip implementation.

- [ ] **Step 4: Replace the local descendant styling with fixed overlay styling**

```css
.settings-help-tooltip {
  position: fixed;
  z-index: 200;
  max-width: min(380px, calc(100vw - 16px));
  pointer-events: none;
}
```

Remove the old relative-parent, `bottom`, `transform`, opacity, and hover-selector rules because visibility is controlled by React state.

- [ ] **Step 5: Run targeted UI tests to verify portal behavior and existing help content**

Run: `npx vitest run src/App.test.tsx -t "help"`

Expected: PASS; focused and hovered help are readable, while hidden help is absent from the accessibility tree.

- [ ] **Step 6: Commit the portal implementation**

```bash
git add src/App.tsx src/App.test.tsx src/styles.css
git commit -m "fix: render help tooltips above scroll containers"
```

### Task 3: Full verification and desktop evidence

**Files:**
- Modify: `openspec/changes/fix-tooltip-overlay/tasks.md`
- Modify: `docs/visual-qa/custom-hotkey-settings-behavior.md` only if the result record is absent

- [ ] **Step 1: Run the complete project check**

Run: `npm run check`

Expected: Prettier, ESLint, Vitest, TypeScript/Vite build, Rust format, Clippy, and Rust tests all pass.

- [ ] **Step 2: Run OpenSpec validation and whitespace check**

Run: `openspec validate fix-tooltip-overlay --strict && git diff --check`

Expected: both commands pass with no output errors.

- [ ] **Step 3: Verify the live macOS desktop window**

Run: `npm run tauri dev`

Expected: in the settings window, hover/focus help at top, bottom, and a long path value; each tooltip is fully readable, remains near its question icon after scrolling, and stays inside the window after resize.

- [ ] **Step 4: Mark completed OpenSpec tasks and commit verification evidence**

```bash
git add openspec/changes/fix-tooltip-overlay/tasks.md docs/visual-qa/custom-hotkey-settings-behavior.md
git commit -m "docs: verify tooltip overlay behavior"
```
