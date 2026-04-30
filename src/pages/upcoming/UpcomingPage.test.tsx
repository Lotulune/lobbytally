// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import { UpcomingPage } from "./UpcomingPage";

describe("UpcomingPage", () => {
  it("renders the upcoming page headline and empty state", () => {
    render(
      <UpcomingPage
        games={[]}
        onOpen={vi.fn()}
        onToggleFollow={vi.fn()}
      />,
    );

    expect(screen.getByRole("heading", { name: "即将上线" })).toBeInTheDocument();
    expect(screen.getByText(/未来发售、即将开放 Demo、或待公布/)).toBeInTheDocument();
    expect(screen.getByText(/还没有符合条件的即将上线游戏/)).toBeInTheDocument();
  });

  it("opens detail and toggles follow from upcoming cards", () => {
    const onOpen = vi.fn();
    const onToggleFollow = vi.fn();

    render(
      <UpcomingPage
        games={[mockDashboard.upcoming[0]]}
        onOpen={onOpen}
        onToggleFollow={onToggleFollow}
      />,
    );

    fireEvent.click(screen.getByRole("img", { name: mockDashboard.upcoming[0]?.name ?? "" }));
    fireEvent.click(screen.getByRole("button", { name: /关注上线/i }));

    expect(onOpen).toHaveBeenCalledWith(mockDashboard.upcoming[0]);
    expect(onToggleFollow).toHaveBeenCalledWith(mockDashboard.upcoming[0]);
  });
});
