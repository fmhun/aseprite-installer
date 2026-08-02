import { StrictMode } from "react";
import { createRoot, hydrateRoot } from "react-dom/client";
import App from "./App";
import { installPlatformSimulationConsole } from "./platformSimulation";
import "./styles.css";

const uninstallPlatformSimulationConsole = installPlatformSimulationConsole();
if (import.meta.hot) {
  import.meta.hot.dispose(uninstallPlatformSimulationConsole);
}

const container = document.getElementById("root")!;
const app = (
  <StrictMode>
    <App />
  </StrictMode>
);

if (container.dataset.prerendered === "true") {
  hydrateRoot(container, app);
} else {
  createRoot(container).render(app);
}
