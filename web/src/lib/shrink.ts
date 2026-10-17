/**
 * Photos made small enough to send, in a format the agents attach. Taken from the
 * Alice PWA's `shrinkImage`, with one change: Claude Code and Codex attach only
 * PNG, JPEG, GIF and WebP, so anything else the browser can decode (an iPhone's
 * HEIC, AVIF, BMP, TIFF) always becomes a JPEG, however small it is.
 */

/** Longest edge after shrinking: above what vision models actually use. */
export const MAX_EDGE = 3072;

/** Images the agents attach, at or under this, are sent untouched unless oversized in pixels. */
export const SHRINK_ABOVE_BYTES = 4 * 1024 * 1024;

const JPEG_QUALITY = 0.85;

/** What Claude Code and Codex attach as an image from its path. */
const ATTACHED = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);

/** The file's type, from its name when the browser gave none (some HEICs on iOS). */
export function imageType(file: { type: string; name: string }): string {
  if (file.type) return file.type;
  return /\.hei[cf]$/i.test(file.name) ? "image/heic" : "";
}

/**
 * Formats worth re-encoding. GIFs would lose their animation and SVGs are not
 * pixels; anything a browser can't decode (HEIC outside Safari) falls through the
 * decode failure in `shrinkImage` and goes as it is.
 */
export function isShrinkable(mime: string): boolean {
  return mime.startsWith("image/") && mime !== "image/gif" && mime !== "image/svg+xml";
}

/** Dimensions scaled so the longest edge is at most `maxEdge`. */
export function fitWithin(width: number, height: number, maxEdge = MAX_EDGE) {
  const scale = Math.min(1, maxEdge / Math.max(width, height));
  return { width: Math.round(width * scale), height: Math.round(height * scale) };
}

/** `IMG_1234.HEIC` → `IMG_1234.jpg`. */
export function jpegName(name: string): string {
  const base = name.replace(/\.[^./\\]+$/, "");
  return `${base || "photo"}.jpg`;
}

/**
 * A JPEG of `file` no larger than `MAX_EDGE` on its long side, or `file` itself
 * when the agents attach it and it's small already, it's not a photo, or it can't
 * be decoded here. Never throws: a failed shrink shouldn't stop an upload that
 * may still fit.
 */
export async function shrinkImage(file: File): Promise<File> {
  const type = imageType(file);
  if (!isShrinkable(type) || typeof createImageBitmap === "undefined") return file;
  const attached = ATTACHED.has(type);
  let bitmap: ImageBitmap;
  try {
    // EXIF orientation applied, so a portrait photo stays portrait.
    bitmap = await createImageBitmap(file, { imageOrientation: "from-image" });
  } catch {
    return file;
  }
  try {
    const oversized = Math.max(bitmap.width, bitmap.height) > MAX_EDGE;
    if (attached && !oversized && file.size <= SHRINK_ABOVE_BYTES) return file;

    const { width, height } = fitWithin(bitmap.width, bitmap.height);
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext("2d");
    if (!ctx) return file;
    // JPEG has no alpha; a transparent PNG would otherwise turn black.
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, width, height);
    ctx.drawImage(bitmap, 0, 0, width, height);

    const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/jpeg", JPEG_QUALITY));
    // A bigger JPEG is still worth it for a format the agents can't attach.
    if (!blob || (attached && blob.size >= file.size)) return file;
    return new File([blob], jpegName(file.name), { type: "image/jpeg", lastModified: file.lastModified });
  } catch {
    return file;
  } finally {
    bitmap.close();
  }
}
