export interface HydrationOptions {
  root?: Element | Document
}

export function hydrate(_options: HydrationOptions = {}): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent("ruvyxa:hydrate"))
  }
}
