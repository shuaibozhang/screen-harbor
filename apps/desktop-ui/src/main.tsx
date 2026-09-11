import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { RecordingControls } from "./RecordingControls";
import { SelectionControls } from "./SelectionControls";
import { I18nProvider } from "./i18n";

const windowKind = new URLSearchParams(window.location.search).get("window");

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <I18nProvider>
      {windowKind === "selection-controls"
        ? <SelectionControls />
        : windowKind === "recording-controls"
          ? <RecordingControls />
          : <App />}
    </I18nProvider>
  </StrictMode>,
);
