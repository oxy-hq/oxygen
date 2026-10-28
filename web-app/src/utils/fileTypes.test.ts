import { describe, expect, it } from "vitest";
import { detectFileType, FileType } from "./fileTypes";

describe("detectFileType", () => {
  describe("ANALYTICS_AGENT (.agentic.yml / .agentic.yaml)", () => {
    it("detects .agentic.yml", () => {
      expect(detectFileType("analytics.agentic.yml")).toBe(FileType.ANALYTICS_AGENT);
    });

    it("detects .agentic.yaml", () => {
      expect(detectFileType("training_coach.agentic.yaml")).toBe(FileType.ANALYTICS_AGENT);
    });

    it("detects nested path with .agentic.yml", () => {
      expect(detectFileType("demo_project/analytics.agentic.yml")).toBe(FileType.ANALYTICS_AGENT);
    });
  });

  describe("no regression on existing types", () => {
    it("detects .automation.yml as AUTOMATION", () => {
      expect(detectFileType("etl.automation.yml")).toBe(FileType.AUTOMATION);
    });

    it("detects .procedure.yml as PROCEDURE (back-compat)", () => {
      expect(detectFileType("etl.procedure.yml")).toBe(FileType.PROCEDURE);
    });

    it("returns DEFAULT for retired .workflow.yml extension", () => {
      expect(detectFileType("etl.workflow.yml")).toBe(FileType.DEFAULT);
    });

    it("returns DEFAULT for the removed .app.yml extension", () => {
      expect(detectFileType("dashboard.app.yml")).toBe(FileType.DEFAULT);
    });

    it("returns DEFAULT for unknown extension", () => {
      expect(detectFileType("notes.txt")).toBe(FileType.DEFAULT);
    });

    it("returns DEFAULT for retired .agent.yml extension", () => {
      expect(detectFileType("default.agent.yml")).toBe(FileType.DEFAULT);
    });
  });
});
