import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { App } from "./App";

describe("App", () => {
  it("renders the compact launcher shell", () => {
    render(<App />);

    expect(screen.getByRole("main", { name: "QuickFox launcher" })).toBeInTheDocument();
    expect(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令")).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "搜索结果" })).toBeInTheDocument();
  });
});
