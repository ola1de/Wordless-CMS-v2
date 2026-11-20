/**
 * Client-side mirror of the server permalink builder
 * (src/modules/permalinks/service.ts). Must stay in sync with it.
 */
import type { Post } from "./types";

function dateParts(iso: string): { year: string; month: string; day: string } {
  const d = new Date(iso);
  return {
    year: String(d.getUTCFullYear()),
    month: String(d.getUTCMonth() + 1).padStart(2, "0"),
    day: String(d.getUTCDate()).padStart(2, "0"),
  };
}

export function postUrl(post: Post, structure: string): string {
  if (post.type === "page") return `/${post.slug}`;
  switch (structure) {
    case "plain":
      return `/?p=${post.id}`;
    case "day-name": {
      const { year, month, day } = dateParts(post.date);
      return `/${year}/${month}/${day}/${post.slug}`;
    }
    case "month-name": {
      const { year, month } = dateParts(post.date);
      return `/${year}/${month}/${post.slug}`;
    }
    case "postname":
    default:
      return `/${post.slug}`;
  }
}
