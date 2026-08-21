/**
 * What the slot renders when nothing is intercepting.
 *
 * A slot with no default is left out of the layout's props entirely, so the
 * layout would have to handle an absent `modal`. Rendering nothing is the
 * simpler contract.
 */
export default function NoModal() {
  return null
}
