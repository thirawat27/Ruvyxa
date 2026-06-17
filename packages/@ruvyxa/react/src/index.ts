export interface HydrationOptions {
  root?: Element | Document
}

export function hydrate(_options: HydrationOptions = {}): void {
  throw new Error("Ruvyxa React hydration is not implemented in the MVP runtime yet.")
}
