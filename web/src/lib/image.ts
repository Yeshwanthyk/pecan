/**
 * Client-side downscale/re-encode for composer image attachments, so a
 * full-resolution phone photo doesn't ship as-is over a slow connection.
 * Never throws: any failure falls back to the original file untouched.
 */

/// Skip compression above this size or long edge.
const MAX_BYTES = 1.5 * 1024 * 1024;
const MAX_LONG_EDGE = 2048;
const JPEG_QUALITY = 0.85;
/// Formats left alone: GIFs may be animated, SVGs are already vector/tiny.
const SKIP_TYPES = new Set(["image/gif", "image/svg+xml"]);

/** Downscales/re-encodes `file` when it's over the size or dimension budget; otherwise returns it unchanged. */
export async function compressImageForUpload(file: File): Promise<File> {
  if (SKIP_TYPES.has(file.type)) return file;
  try {
    const bitmap = await createImageBitmap(file);
    const longEdge = Math.max(bitmap.width, bitmap.height);
    if (file.size <= MAX_BYTES && longEdge <= MAX_LONG_EDGE) {
      bitmap.close?.();
      return file;
    }
    const scale = Math.min(1, MAX_LONG_EDGE / longEdge);
    const width = Math.max(1, Math.round(bitmap.width * scale));
    const height = Math.max(1, Math.round(bitmap.height * scale));
    const blob = await drawToBlob(bitmap, width, height);
    bitmap.close?.();
    if (!blob || blob.size >= file.size) return file;
    const name = file.name.replace(/\.\w+$/, "") + ".jpg";
    return new File([blob], name, { type: "image/jpeg", lastModified: Date.now() });
  } catch {
    return file;
  }
}

/** Draws a bitmap at the target size and encodes it as JPEG, preferring OffscreenCanvas. */
async function drawToBlob(bitmap: ImageBitmap, width: number, height: number): Promise<Blob | null> {
  if (typeof OffscreenCanvas !== "undefined") {
    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext("2d");
    if (!ctx) return null;
    ctx.drawImage(bitmap, 0, 0, width, height);
    return canvas.convertToBlob({ type: "image/jpeg", quality: JPEG_QUALITY });
  }
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  ctx.drawImage(bitmap, 0, 0, width, height);
  return new Promise((resolve) => canvas.toBlob(resolve, "image/jpeg", JPEG_QUALITY));
}
