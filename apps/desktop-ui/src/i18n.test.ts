import { describe, expect, it } from "vitest";
import { localizeRelativeTime, translate } from "./i18n";

describe("i18n", () => {
  it("keeps Simplified Chinese as the source language", () => {
    expect(translate("zh-CN", "开始录屏")).toBe("开始录屏");
  });

  it("translates English strings and interpolates values", () => {
    expect(translate("en", "{countdown} 秒后开始", { countdown: 3 })).toBe("Starting in 3 sec");
  });

  it("localizes relative timestamps returned by the backend", () => {
    expect(localizeRelativeTime("en", "1 小时前")).toBe("1 hour ago");
    expect(localizeRelativeTime("en", "3 天前")).toBe("3 days ago");
  });
});
