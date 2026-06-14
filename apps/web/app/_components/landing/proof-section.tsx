import { type LandingCopy } from "./data";
import { renderWithDevoMark } from "./devo-word";

type ProofSectionProps = {
  rows: LandingCopy["proofRows"];
};

export function ProofSection({ rows }: ProofSectionProps) {
  return (
    <section className="border-y border-white/10 bg-[#0a0f14]">
      <div className="mx-auto grid max-w-7xl gap-0 px-5 py-14 sm:px-8 lg:grid-cols-3 lg:px-10">
        {rows.map((item) => (
          <article
            className="border-t border-white/10 py-8 first:border-t-0 lg:border-l lg:border-t-0 lg:px-8 lg:py-0 lg:first:border-l-0 lg:first:pl-0"
            key={item.title}
          >
            <p className="text-xs font-semibold uppercase tracking-[0.18em] text-orange-300/80">
              {item.eyebrow}
            </p>
            <h2 className="mt-4 text-2xl font-semibold tracking-normal text-white">
              {renderWithDevoMark(item.title)}
            </h2>
            <p className="mt-4 text-base leading-7 text-white/58">
              {renderWithDevoMark(item.body)}
            </p>
          </article>
        ))}
      </div>
    </section>
  );
}
