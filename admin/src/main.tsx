import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { applyDensity, applyTheme, readCachedDensity, readCachedTheme } from "./lib/theme";
import "./styles.css";

// P4: apply the cached dashboard theme + density before first paint so
// the admin never flashes the default theme. App refines the theme from
// the user profile once /api/auth/me resolves.
applyTheme(readCachedTheme() ?? "teal");
applyDensity(readCachedDensity());

const root = document.getElementById("root");
if (!root) throw new Error("Admin root element not found.");
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
