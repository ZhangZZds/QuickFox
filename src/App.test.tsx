import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";

describe("App", () => {
  it("renders the compact launcher shell", () => {
    render(<App />);

    expect(screen.getByRole("main", { name: "QuickFox launcher" })).toBeInTheDocument();
    expect(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令")).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "搜索结果" })).toBeInTheDocument();
  });

  it("filters results from the search input and marks the selected result", () => {
    render(<App />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "down" },
    });

    expect(screen.getByText("Downloads")).toBeInTheDocument();
    expect(screen.queryByText("Documents")).not.toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Downloads/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("moves selection with arrow keys and executes the selected primary action with Enter", () => {
    const onExecuteAction = vi.fn();
    render(<App onExecuteAction={onExecuteAction} />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onExecuteAction).toHaveBeenCalledWith({
      type: "openPath",
      path: "~/Downloads",
    });
  });

  it("closes the launcher with Esc without executing an action", () => {
    const onClose = vi.fn();
    const onExecuteAction = vi.fn();
    render(<App onClose={onClose} onExecuteAction={onExecuteAction} />);

    fireEvent.keyDown(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      key: "Escape",
    });

    expect(onClose).toHaveBeenCalledOnce();
    expect(onExecuteAction).not.toHaveBeenCalled();
  });

  it("opens the action menu from context menu and executes secondary actions", () => {
    const onExecuteAction = vi.fn();
    render(<App onExecuteAction={onExecuteAction} />);

    fireEvent.contextMenu(screen.getByRole("option", { name: /Documents/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "复制路径" }));

    expect(onExecuteAction).toHaveBeenCalledWith({
      type: "copyText",
      text: "~/Documents",
    });
  });

  it("shows command preview when command mode is enabled", () => {
    render(<App commandEnabled />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "> git status" },
    });

    expect(screen.getByRole("region", { name: "命令预览" })).toBeInTheDocument();
    expect(screen.getByText("git status")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认执行" })).toBeInTheDocument();
  });

  it("renders the basic settings view", () => {
    render(<App initialView="settings" />);

    expect(screen.getByRole("form", { name: "基础设置" })).toBeInTheDocument();
    expect(screen.getByLabelText("索引目录")).toBeInTheDocument();
    expect(screen.getByLabelText("正则前缀")).toHaveValue("re:");
    expect(screen.getByLabelText("命令执行")).not.toBeChecked();
    expect(screen.getByLabelText("命令历史条数")).toHaveValue(15);
  });
});
