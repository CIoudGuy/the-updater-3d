import { mount } from "svelte";
import "./styles/global.css";
import Tray from "./Tray.svelte";
import { installAppLike } from "./lib/applike";

installAppLike(document);

export default mount(Tray, { target: document.getElementById("app")! });
