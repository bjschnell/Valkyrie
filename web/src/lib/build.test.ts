import { describe, expect, it } from "vitest";
import { buildOf } from "./build";

describe("buildOf", () => {
  it("names a build by its hashed files, in the order the server does", () => {
    expect(
      buildOf([
        "https://thor.example.ts.net/assets/index-BNAWTcRn.css",
        "https://thor.example.ts.net/assets/index-76KRp27S.js",
        "https://thor.example.ts.net/manifest.webmanifest",
        "/icons/icon-192.png",
      ]),
    ).toBe("/assets/index-76KRp27S.js /assets/index-BNAWTcRn.css");
  });

  it("is empty on a dev server, which has no hashed files", () => {
    expect(buildOf(["http://localhost:5173/src/main.tsx", "http://localhost:5173/@vite/client"])).toBe("");
  });
});
