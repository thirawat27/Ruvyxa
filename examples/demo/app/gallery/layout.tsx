/**
 * The level an interception replaces.
 *
 * `children` is whatever page the URL names; `modal` is the parallel-route slot
 * that `@modal` fills. A soft navigation to `/gallery/photo` swaps only the
 * slot, so this layout and the page underneath keep their state — which is the
 * whole point of an intercepting route, and the thing a plain navigation cannot
 * do.
 */
export default function GalleryLayout({
  children,
  modal,
}: Readonly<{ children: React.ReactNode; modal?: React.ReactNode }>) {
  return (
    <div className="gallery-shell">
      {children}
      {modal}
    </div>
  )
}
