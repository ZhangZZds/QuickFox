# Result Activation Behavior Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make left-click and Enter activate search results through the same primary action path.

**Architecture:** The frontend keeps result activation as a single UI concern and executes the `primaryAction` supplied by Rust core. Cross-platform file, directory, and application opening remains in Rust/Tauri opener.

**Tech Stack:** React, TypeScript, Vitest, Testing Library, Tauri action IPC.

---

## File Structure

- Modify `src/App.test.tsx`: add behavior tests for left-click result activation and Enter/click parity.
- Modify `src/App.tsx`: extract a result activation helper and wire result row `onClick`.
- Modify `openspec/changes/define-result-activation-behavior/tasks.md`: mark completed tasks as implementation progresses.

## Task 1: Add Failing Frontend Tests

**Files:**

- Modify: `src/App.test.tsx`

- [ ] **Step 1: Add a test for left-clicking a file result**

Add a test near existing result activation tests:

```tsx
it("executes the primary action when left-clicking a file result", async () => {
  const onExecuteAction = vi.fn();
  vi.mocked(search).mockResolvedValueOnce(typedResults);
  render(<App onExecuteAction={onExecuteAction} />);

  fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
    target: { value: "typed" },
  });

  fireEvent.click(await screen.findByRole("option", { name: /report\.md/ }));

  expect(onExecuteAction).toHaveBeenCalledWith({
    type: "openPath",
    path: "/tmp/report.md",
  });
});
```

- [ ] **Step 2: Add tests for directory and application left-click activation**

Use `typedResults` and assert `/tmp/Documents` and `/Applications/Codex.app` primary actions.

- [ ] **Step 3: Add a test for Enter/click parity**

Render `fileResults`, select Downloads by ArrowDown, press Enter, then render again and click Downloads. Assert both calls receive `{ type: "openPath", path: "/tmp/Downloads" }`.

- [ ] **Step 4: Run target tests and verify failure**

Run: `npm test -- src/App.test.tsx --runInBand`

Expected before implementation: at least one new click activation assertion fails because result rows do not call `onExecuteAction` on left-click.

## Task 2: Implement Unified Activation

**Files:**

- Modify: `src/App.tsx`

- [ ] **Step 1: Extract a helper that activates a provided result**

Add a helper near `executeSelected`:

```tsx
const executeResult = async (result: LauncherResult, executedInput = query.trim()) => {
  await onExecuteAction(result.primaryAction);
  if (executedInput) {
    await recordInputHistory(executedInput);
  }
};
```

- [ ] **Step 2: Reuse helper from Enter path**

Replace the selected-result branch in `executeSelected` with:

```tsx
if (selectedResult) {
  await executeResult(selectedResult, executedInput);
}
```

- [ ] **Step 3: Wire result row left-click**

On each result `<li>`, add:

```tsx
onClick={() => {
  setSelectedIndex(index);
  void executeResult(result);
}}
```

Keep existing `onContextMenu`, `onMouseEnter`, and `onMouseLeave` behavior.

- [ ] **Step 4: Run target tests and verify pass**

Run: `npm test -- src/App.test.tsx --runInBand`

Expected after implementation: click activation and existing launcher tests pass.

## Task 3: Verify and Mark Tasks

**Files:**

- Modify: `openspec/changes/define-result-activation-behavior/tasks.md`

- [ ] **Step 1: Run frontend checks**

Run:

```bash
npm run build
npm run lint
npm run format
npm test -- src/App.test.tsx
```

Expected: all commands exit 0.

- [ ] **Step 2: Mark OpenSpec tasks complete**

Update every checkbox in `openspec/changes/define-result-activation-behavior/tasks.md` from `- [ ]` to `- [x]` only after the matching work and verification have completed.

- [ ] **Step 3: Review diff**

Run: `git diff -- src/App.tsx src/App.test.tsx openspec/changes/define-result-activation-behavior docs/superpowers`

Expected: only this change's files are modified.
