import { afterEach, describe, expect, it, vi } from "vitest";
import Home from "../src/app/page";
import { localModelLocation } from "../src/ui/JudgeDoor";

afterEach(() => vi.unstubAllEnvs());

describe("deployment-specific local model wording", () => {
  it("passes hosted mode only when PRU_HOSTED_DOOR is enabled", () => {
    vi.stubEnv("PRU_HOSTED_DOOR", "1");
    expect(Home().props.hosted).toBe(true);

    vi.stubEnv("PRU_HOSTED_DOOR", "0");
    expect(Home().props.hosted).toBe(false);
  });

  it("names the server in hosted mode and the PC in the test build", () => {
    expect(localModelLocation(true)).toBe(
      "this server's own model, never sent to a model provider",
    );
    expect(localModelLocation(false)).toBe(
      "this PC's own model, never sent to a model provider",
    );
  });
});
