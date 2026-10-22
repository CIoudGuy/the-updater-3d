import { mount } from "svelte";
import "./styles/global.css";
import Tray from "./Tray.svelte";

export default mount(Tray, { target: document.getElementById("app")! });
