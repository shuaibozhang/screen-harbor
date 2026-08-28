import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { RecordingControls } from "./RecordingControls";
import { SelectionControls } from "./SelectionControls";

const windowKind = new URLSearchParams(window.location.search).get("window");

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {windowKind === "selection-controls"
      ? <SelectionControls />
      : windowKind === "recording-controls"
        ? <RecordingControls />
        : <App />}
  </StrictMode>,
);
