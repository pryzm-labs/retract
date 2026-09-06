import { safeMessages, type ActionDescriptor, type ExpectedEffect } from "../providers/contract";

const effects: Record<ExpectedEffect, string> = {
  removed_for_all_participants: "Remove for all participants",
  removed_for_current_account_only: "Remove only for your account",
  public_content_removed: "Remove public content",
  membership_removed: "Remove membership",
  container_destroyed: "Permanently destroy the conversation",
  local_import_removed: "Remove the local import",
  manual_or_unknown: "Manual action or unverified effect"
};

export function diagnosticMessage(code: string): string {
  return Object.hasOwn(safeMessages, code)
    ? safeMessages[code as keyof typeof safeMessages]
    : "This action could not be completed. Review its status before retrying.";
}

export function ActionEffects({ descriptors, label, targetCounts }: {
  descriptors: ActionDescriptor[]; label: string; targetCounts?: number[];
}) {
  return <ol className="action-effects" aria-label={label}>
    {descriptors.map((descriptor, index) => <li key={`${descriptor.id}-${index}`}>
      <span>{effects[descriptor.effect]}</span>
      {targetCounts && <span> · {targetCounts[index]} reviewed {targetCounts[index] === 1 ? "target" : "targets"}</span>}
      {descriptor.availability === "live_preflight_required" && <small>Live permission check required</small>}
      {descriptor.availability === "manual_only" && <small>Manual action only</small>}
      {descriptor.availability === "unavailable" && <small>Unavailable · {diagnosticMessage(descriptor.unavailableReason?.code ?? "")}</small>}
    </li>)}
  </ol>;
}
