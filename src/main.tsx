import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./components/ui/error-boundary";
import { initTheme } from "./lib/theme";
import "./styles/index.css";

initTheme();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary reloadOnRetry>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
