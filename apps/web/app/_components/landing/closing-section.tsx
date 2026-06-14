import Link from "next/link";

import { type LandingCopy } from "./data";
import { renderWithDevoMark } from "./devo-word";
import { ArrowIcon } from "./icons";

type ClosingSectionProps = {
  copy: LandingCopy["closing"];
  docsHref: string;
};

export function ClosingSection({ copy, docsHref }: ClosingSectionProps) {
  return (
    <section
      className="scroll-mt-6 bg-[radial-gradient(circle_at_82%_12%,rgb(255_148_31_/_16%),transparent_24rem),#070a0f] px-5 py-20 sm:px-8 lg:px-10"
      id="contact"
    >
      <div className="mx-auto flex max-w-7xl flex-col gap-8 border-t border-white/12 pt-12 lg:flex-row lg:items-end lg:justify-between">
        <div>
          <p className="text-xs font-extrabold uppercase tracking-[0.18em] text-[#ffb057]/85">
            {copy.kicker}
          </p>
          <h2 className="mt-4 max-w-2xl text-4xl font-semibold tracking-normal text-white sm:text-5xl">
            {copy.title}
          </h2>
          <p className="mt-5 max-w-xl text-lg leading-8 text-white/58">
            {renderWithDevoMark(copy.bodyBeforeEmail)}{" "}
            <a
              className="text-orange-200 underline-offset-4 hover:underline"
              href="mailto:devo@devo.7df.ai"
            >
              devo@devo.7df.ai
            </a>
            .
          </p>
          <p className="mt-4 text-sm font-semibold uppercase tracking-[0.16em] text-white/42">
            {copy.locationLabel}:{" "}
            <span className="font-bold normal-case tracking-normal text-white/70">
              {copy.location}
            </span>
          </p>
        </div>
        <div className="flex flex-col gap-3 sm:flex-row">
          <Link
            className="inline-flex min-h-12 items-center justify-center gap-2 bg-[#ff941f] px-5 text-sm font-bold text-[#080a0e] transition hover:-translate-y-px hover:bg-[#ffb45f]"
            href={docsHref}
          >
            {copy.openDocs}
            <ArrowIcon />
          </Link>
          <a
            className="inline-flex min-h-12 items-center justify-center gap-2 border border-white/20 bg-white/7 px-5 text-sm font-bold text-white transition hover:-translate-y-px hover:border-white/35 hover:bg-white/12"
            href="https://github.com/7df-lab/devo"
            rel="noreferrer"
            target="_blank"
          >
            {copy.viewGithub}
          </a>
        </div>
      </div>
    </section>
  );
}
