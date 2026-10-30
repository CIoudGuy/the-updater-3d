// The words of the "Close the app to continue" dialog, which depend on what the job does.
import type { Job } from "./types";

export interface BlockedText {
  title: string;
  lead: string;
  info: string;
  action: string;
}

export function blockedText(j: Pick<Job, "kind" | "name" | "toVersion" | "fromVersion" | "blockedBy" | "blockedByFiles">, otherJobs: number): BlockedText {
  const who = j.blockedBy.length ? j.blockedBy.join(", ") : j.name;
  const others = otherJobs > 0 ? " The other jobs keep going." : "";
  if (j.kind === "uninstall") {
    return {
      title: `Close ${j.name} to uninstall it`,
      lead: `${j.name} is still open. Save your work, then close it so it can be uninstalled.${others}`,
      info: `Uninstalls by itself when ${who} closes`,
      action: "Close and uninstall",
    };
  }
  const verb = j.kind === "install" ? "install" : j.kind === "rollback" ? "roll back" : j.kind === "repair" ? "repair" : "update";
  // Other apps hold its files: those are what the button closes, not the app itself.
  if (j.blockedByFiles) {
    const many = j.blockedBy.length > 1;
    return {
      title: `Close ${who} to ${verb} ${j.name}`,
      lead: `${who} ${many ? "are" : "is"} using ${j.name}'s files, so ${j.name} ${j.toVersion} can't install yet. Save your work, then close ${many ? "them" : "it"}.`,
      info: `Installs by itself when ${who} ${many ? "close" : "closes"}`,
      action: `Close and ${verb}`,
    };
  }
  return {
    title: `Close ${j.name} to continue`,
    lead: `${j.name} ${j.toVersion} is downloaded and ready to install. Save your work, then close ${j.name}.${otherJobs > 0 ? " The other updates keep going." : ""}`,
    info: `Installs by itself when ${who} closes`,
    action: `Close and ${verb}`,
  };
}
