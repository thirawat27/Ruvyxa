import { useId } from 'react'
import type { ReactElement, ReactNode } from 'react'

export interface AnswerSource {
  name: string
  url: string
}

interface AnswerBaseProps {
  /** The exact question answered by the visible content. */
  question: string
  /** Stable anchor ID. React creates an SSR-safe ID when omitted. */
  id?: string
  /** Visible citations supporting the answer. */
  sources?: readonly AnswerSource[]
  /** Localized heading shown above citations. @default "Sources" */
  sourcesLabel?: ReactNode
  className?: string
}

export type AnswerProps = AnswerBaseProps &
  (
    | { /** Plain or rich answer content. */ answer: ReactNode; children?: never }
    | { answer?: never; children: ReactNode }
  )

/**
 * Renders a concise, visible answer with citeable sources and Schema.org
 * Question/Answer microdata. It intentionally does not claim FAQ/Q&A rich-result eligibility.
 */
export function Answer(props: AnswerProps): ReactElement {
  const { question, id, sources, sourcesLabel = 'Sources', className } = props
  const content = 'answer' in props ? props.answer : props.children
  const generatedId = useId()
  const headingId = id ?? `ruvyxa-answer-${generatedId.replaceAll(':', '')}`

  return (
    <section
      className={className}
      data-ruvyxa-answer=""
      aria-labelledby={headingId}
      itemScope
      itemType="https://schema.org/Question"
    >
      <h2 id={headingId} itemProp="name">
        {question}
      </h2>
      <div itemProp="acceptedAnswer" itemScope itemType="https://schema.org/Answer">
        <div itemProp="text">{content}</div>
        {sources?.length ? (
          <footer data-ruvyxa-answer-sources="">
            <p>
              <strong>{sourcesLabel}</strong>
            </p>
            <ol>
              {sources.map((source) => (
                <li key={`${source.url}\u0000${source.name}`}>
                  <a href={source.url} itemProp="citation">
                    {source.name}
                  </a>
                </li>
              ))}
            </ol>
          </footer>
        ) : null}
      </div>
    </section>
  )
}
