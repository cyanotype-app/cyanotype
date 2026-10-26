import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/** Class names joined, with later Tailwind utilities winning over earlier ones (`cn("p-2", big && "p-4")`). */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
