import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { demoSearch, demoSnapshot } from "../demo";
import { ResultsList, messageKey } from "./ResultsList";

describe("Discord result completion", () => {
  it("keeps archive evidence visible but prevents repeating a confirmed deletion", async () => {
    const snapshot = await demoSnapshot();
    const result = await demoSearch({
      query: "", conversations: [], chatKinds: [], contentKinds: [], direction: "any",
      excludePinned: false, limit: 1
    });
    const message = result.messages[0];
    render(<ResultsList
      provider="discord"
      messages={[message]}
      chats={snapshot.chats}
      selectedKeys={new Set()}
      completedKeys={new Set([messageKey(message)])}
      loading={false}
      refreshing={false}
      query=""
      privacyScan={false}
      truncated={false}
      onToggle={vi.fn()}
      onToggleAll={vi.fn()}
    />);

    expect(screen.getByText("Deleted / absent")).toBeVisible();
    expect(screen.getByLabelText(`Select message from ${message.senderName}`)).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: /result/ })).toBeDisabled();
  });
});
