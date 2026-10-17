import { describe, expect, it } from "vitest";
import { fitWithin, imageType, isShrinkable, jpegName, MAX_EDGE, shrinkImage } from "./shrink";

describe("fitWithin", () => {
  it("scales the long edge down to the cap, keeping the aspect", () => {
    expect(fitWithin(8160, 6120)).toEqual({ width: MAX_EDGE, height: 2304 });
    expect(fitWithin(6120, 8160)).toEqual({ width: 2304, height: MAX_EDGE });
  });

  it("never scales up", () => {
    expect(fitWithin(1200, 900)).toEqual({ width: 1200, height: 900 });
  });
});

describe("isShrinkable", () => {
  it("takes photos but not animations or vectors", () => {
    expect(isShrinkable("image/jpeg")).toBe(true);
    expect(isShrinkable("image/heic")).toBe(true);
    expect(isShrinkable("image/png")).toBe(true);
    expect(isShrinkable("image/gif")).toBe(false);
    expect(isShrinkable("image/svg+xml")).toBe(false);
    expect(isShrinkable("application/pdf")).toBe(false);
  });
});

describe("imageType", () => {
  it("names a HEIC the browser gave no type", () => {
    expect(imageType({ type: "", name: "IMG_1234.HEIC" })).toBe("image/heic");
    expect(imageType({ type: "", name: "notes.txt" })).toBe("");
    expect(imageType({ type: "image/png", name: "a.heic" })).toBe("image/png");
  });
});

describe("jpegName", () => {
  it("swaps the extension", () => {
    expect(jpegName("IMG_1234.HEIC")).toBe("IMG_1234.jpg");
    expect(jpegName("shot.png")).toBe("shot.jpg");
    expect(jpegName("noext")).toBe("noext.jpg");
    expect(jpegName(".png")).toBe("photo.jpg");
  });
});

describe("shrinkImage", () => {
  it("passes non-images through untouched", async () => {
    const pdf = new File([new Uint8Array(10)], "a.pdf", { type: "application/pdf" });
    expect(await shrinkImage(pdf)).toBe(pdf);
  });
});
