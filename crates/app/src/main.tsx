import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import { ErrorBoundary } from "./components/error-boundary";
import "./globals.css";

const rootEl = document.getElementById("root");
if (!rootEl) throw new Error("missing #root");

ReactDOM.createRoot(rootEl).render(
  <React.StrictMode>
    <ErrorBoundary area="OpenForge">
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
