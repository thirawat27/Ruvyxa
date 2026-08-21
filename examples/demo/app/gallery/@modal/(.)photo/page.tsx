import { useRouter } from '@ruvyxa/react'

/**
 * The overlay `/gallery/photo` opens as, on a soft navigation only.
 *
 * `router.back()` closes it by popping the history entry the interception
 * pushed, which returns the URL to the page still mounted underneath.
 */
export default function PhotoModal() {
  const router = useRouter()
  return (
    <dialog className="gallery-modal" open aria-label="Photo">
      <h2>Photo</h2>
      <p>
        Rendered from <code>app/gallery/@modal/(.)photo/page.tsx</code> into the layout&apos;s{' '}
        <code>modal</code> slot. The gallery is still mounted behind this.
      </p>
      <button type="button" onClick={() => router.back()}>
        Close
      </button>
    </dialog>
  )
}
