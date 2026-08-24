import { type CxOptions, cx } from "class-variance-authority";
import { twMerge } from "tailwind-merge";

/** Merges Tailwind class lists with last-wins conflict resolution. */
export function cn(...inputs: CxOptions) {
  return twMerge(cx(inputs));
}
