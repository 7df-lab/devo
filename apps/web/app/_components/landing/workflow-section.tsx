import Image from "next/image";

import { type LandingCopy } from "./data";
import { renderWithDevoMark } from "./devo-word";

type WorkflowSectionProps = {
  copy: LandingCopy["workflow"];
};

export function WorkflowSection({ copy }: WorkflowSectionProps) {
  return (
    <section className="bg-[#070a0f] px-5 py-20 sm:px-8 lg:px-10">
      <div className="mx-auto grid max-w-7xl gap-10 lg:grid-cols-[0.88fr_1.12fr] lg:items-center">
        <div>
          <p className="text-xs font-extrabold uppercase tracking-[0.18em] text-[#ffb057]/85">
            {copy.kicker}
          </p>
          <h2 className="mt-4 max-w-xl text-4xl font-semibold tracking-normal text-white sm:text-5xl">
            {renderWithDevoMark(copy.title)}
          </h2>
          <p className="mt-6 max-w-xl text-lg leading-8 text-white/62">
            {renderWithDevoMark(copy.body)}
          </p>
        </div>
        <div className="relative flex min-h-[27rem] items-center justify-end md:min-h-[clamp(25rem,52vw,43rem)]">
          <Image
            src="/cli.png"
            alt={copy.imageAlt}
            width={1104}
            height={621}
            className="h-auto w-[min(92vw,34rem)] border border-white/12 shadow-[0_2rem_5rem_rgb(0_0_0_/_42%)] md:w-[min(78vw,43rem)]"
          />
        </div>
      </div>
    </section>
  );
}
