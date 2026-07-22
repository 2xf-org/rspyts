import { greet } from "../api.js";

export function excitedGreeting(name: string): string {
  return greet(name).message.toUpperCase();
}
