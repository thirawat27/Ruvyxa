interface Props {
  items: string[]
  extra: Record<string, unknown>
}

export default function Page({ items, extra }: Props) {
  return (
    <>
      <UI.Card data-kind="result" {...extra}>
        {items.map((item) => <svg:path data-value={item} />)}
      </UI.Card>
    </>
  )
}
