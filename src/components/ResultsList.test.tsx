import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { demoSearch, demoSnapshot } from "../demo";
import { ResultsList, messageKey } from "./ResultsList";

describe("Discord result completion", () => {
  it.each(["candidate_limit", "incomplete"] as const)("focused never presents an empty %s privacy scan as clean", completion => {
    render(<ResultsList provider="telegram" messages={[]} chats={[]} selectedKeys={new Set()} completedKeys={new Set()} loading={false} refreshing={false} query="" privacyScan truncated completion={completion} onToggle={vi.fn()} onToggleAll={vi.fn()} />);
    expect(screen.getByRole("status")).toHaveTextContent("Search incomplete");
    expect(screen.queryByText("No sensitive information found")).not.toBeInTheDocument();
    expect(screen.getByText(/Showing up to the first 500 matches/)).toBeVisible();
    expect(screen.getByRole("checkbox")).toBeDisabled();
  });

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
