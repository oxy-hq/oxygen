// @vitest-environment jsdom

import { describe, expect, it } from "vitest";
import { latestDistinctQuestions, type RecentQuestion } from "./HomeRecentQuestions";

const q = (id: string, title: string, day: number): RecentQuestion => ({
  id,
  title,
  timestamp: new Date(Date.UTC(2026, 9, day)).toISOString()
});

describe("latestDistinctQuestions", () => {
  it("shows a question asked three times once, by its latest thread", () => {
    const rows = latestDistinctQuestions([
      q("a", "give recommendations for staffs based on sales", 1),
      q("b", "Give recommendations for staffs  based on sales ", 3),
      q("c", "give recommendations for staffs based on sales", 2),
      q("d", "What were sales yesterday?", 4),
      q("e", "Which store is slowest?", 5)
    ]);
    expect(rows.map((r) => r.id)).toEqual(["e", "d", "b"]);
  });

  it("keeps the newest three when every question is different", () => {
    const rows = latestDistinctQuestions([
      q("a", "one", 1),
      q("b", "two", 2),
      q("c", "three", 3),
      q("d", "four", 4)
    ]);
    expect(rows.map((r) => r.id)).toEqual(["d", "c", "b"]);
  });
});
