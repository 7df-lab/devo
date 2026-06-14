import Image from "next/image";

import { type LandingCopy } from "./data";
import { renderWithDevoMark } from "./devo-word";

type ComparisonSectionProps = {
  copy: LandingCopy["comparison"];
};

type ProductMarkProps = {
  product: string;
};

const productLogos: Record<string, string> = {
  Devo: "/devo-mark.svg",
  "Claude Code": "/brand/claude-code.svg",
  Droid: "/brand/droid.svg",
  OpenCode: "/brand/opencode.svg",
};

const statusTone = {
  yes: {
    text: "text-emerald-200",
    dot: "bg-emerald-300 shadow-[0_0_1.25rem_rgb(110_231_183_/_36%)]",
  },
  partial: {
    text: "text-amber-200",
    dot: "bg-amber-300 shadow-[0_0_1.25rem_rgb(252_211_77_/_30%)]",
  },
  no: {
    text: "text-white/46",
    dot: "bg-white/24",
  },
} as const;

function ProductMark({ product }: ProductMarkProps) {
  const src = productLogos[product];

  if (src) {
    return (
      <Image
        alt=""
        className="h-6 w-6 object-contain"
        height={24}
        src={src}
        width={24}
      />
    );
  }

  return (
    <span className="inline-flex h-6 w-6 items-center justify-center rounded-full border border-white/16 text-xs font-bold text-white/70">
      {product.slice(0, 1)}
    </span>
  );
}

export function ComparisonSection({ copy }: ComparisonSectionProps) {
  return (
    <section className="bg-[#070a0f] px-5 py-20 sm:px-8 lg:px-10">
      <div className="mx-auto max-w-7xl">
        <div className="flex flex-col gap-6 border-b border-white/12 pb-8 lg:flex-row lg:items-end lg:justify-between">
          <div>
            <p className="text-xs font-extrabold uppercase tracking-[0.18em] text-[#ffb057]/85">
              {copy.kicker}
            </p>
            <h2 className="mt-4 max-w-3xl text-4xl font-semibold tracking-normal text-white sm:text-5xl">
              {renderWithDevoMark(copy.title)}
            </h2>
          </div>
        </div>

        <div className="mt-8 overflow-x-auto border border-white/12 bg-[#0a0f14] shadow-[0_2rem_5rem_rgb(0_0_0_/_28%)]">
          <table className="min-w-[72rem] border-collapse text-left">
            <thead>
              <tr className="border-b border-white/12">
                <th
                  className="sticky left-0 z-20 w-56 bg-[#0a0f14] px-5 py-5 text-xs font-bold uppercase tracking-[0.16em] text-white/42"
                  scope="col"
                >
                  {copy.capabilityLabel}
                </th>
                {copy.products.map((product, index) => (
                  <th
                    className={[
                      "w-60 px-5 py-5 text-sm font-semibold text-white",
                      index === 0 ? "bg-[#111822]" : "",
                    ].join(" ")}
                    key={product}
                    scope="col"
                  >
                    <span className="flex items-center gap-2.5 text-base">
                      <span className="inline-flex h-8 w-8 items-center justify-center rounded-lg border border-white/12 bg-white/[0.04] text-white/82">
                        <ProductMark product={product} />
                      </span>
                      {product}
                    </span>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {copy.rows.map((row) => (
                <tr
                  className="border-t border-white/8 align-top transition-colors hover:bg-white/[0.025]"
                  key={row.capability}
                >
                  <th
                    className="sticky left-0 z-10 w-56 bg-[#0a0f14] px-5 py-5 text-sm font-semibold leading-6 text-white"
                    scope="row"
                  >
                    {row.capability}
                  </th>
                  {row.products.map((product, index) => {
                    const tone = statusTone[product.status];

                    return (
                      <td
                        className={[
                          "w-60 px-5 py-5",
                          index === 0
                            ? "bg-[linear-gradient(180deg,rgb(255_148_31_/_9%),rgb(255_148_31_/_3%))]"
                            : "",
                        ].join(" ")}
                        key={`${row.capability}-${index}`}
                      >
                        <div
                          className={`inline-flex items-center gap-2 text-sm font-semibold ${tone.text}`}
                        >
                          <span
                            aria-hidden="true"
                            className={`h-2 w-2 rounded-full ${tone.dot}`}
                          />
                          {copy.statusLabels[product.status]}
                        </div>
                        <p className="mt-2 text-xs leading-5 text-white/48">
                          {product.evidence}
                        </p>
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </section>
  );
}
