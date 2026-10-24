// Where a route the backend sends ("navigate", take_route) takes the window. The backend sends:
//   an app's page          installed/<encodeURIComponent(app id)>
//   several updates        updates     (Installed, filtered to the updates)
//   the tray's Update all  update-all  (Home, then the update starts)
//   any page of the router queue, home, installed, settings/about, ...
import type { InstalledFilter } from "./ui.svelte";

export interface Landing {
  /** the router path to open */
  path: string;
  /** Installed opens with this filter (and every group) */
  filter?: InstalledFilter;
  /** start "Update all" once the page is open */
  updateAll?: boolean;
}

export function landing(route: string): Landing {
  const path = route.replace(/^#?\/?/, "");
  if (path === "update-all") return { path: "home", updateAll: true };
  if (path === "updates") return { path: "installed", filter: "updates" };
  return { path };
}
