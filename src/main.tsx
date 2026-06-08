import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles.css";
import { currentWindowLabel } from "./tauriClient";

const initialView =
  new URLSearchParams(window.location.search).get("view") === "settings" ||
  currentWindowLabel() === "settings"
    ? "settings"
    : "launcher";

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    <App initialView={initialView} />
  </StrictMode>,
);
