import { mount } from "svelte";
import "./styles/global.css";
import App from "./App.svelte";
import { applyZoom, cachedZoom } from "./lib/zoom";

// Use the zoom of the last run straight away; the saved setting confirms it once the snapshot arrives.
const z = cachedZoom();
if (z !== 1) void applyZoom(z);

export default mount(App, { target: document.getElementById("app")! });
