import { type ReactNode } from "react";

import { type LandingCopy } from "./data";
import { renderWithDevoMark } from "./devo-word";

type EnterpriseSectionProps = {
  copy: LandingCopy["enterprise"];
};

type EnterpriseCopy = LandingCopy["enterprise"];
type EnterpriseFeature = EnterpriseCopy["features"][number];

const modelColors = ["#60A5FA", "#ff941f", "#7dd3fc", "#facc15", "#a78bfa"];

const usageBars = [
  [36, 22, 12, 8, 6],
  [42, 24, 13, 10, 8],
  [39, 20, 15, 9, 7],
  [45, 25, 14, 11, 9],
  [50, 27, 16, 12, 10],
  [44, 23, 15, 10, 8],
  [41, 22, 13, 9, 7],
  [48, 27, 17, 12, 9],
  [55, 30, 18, 13, 10],
  [61, 34, 20, 15, 12],
  [58, 31, 19, 14, 11],
  [52, 28, 17, 12, 9],
  [47, 25, 16, 11, 8],
  [43, 23, 14, 10, 7],
];

function MiniTrend() {
  return (
    <svg aria-hidden="true" className="h-6 w-20 text-[#60A5FA]" viewBox="0 0 80 24">
      <path
        d="M2 18 13 15 24 17 35 9 46 12 57 7 68 10 78 5"
        fill="none"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="2"
      />
    </svg>
  );
}

function DashboardFrame({
  children,
  copy,
}: {
  children: ReactNode;
  copy: EnterpriseCopy["dashboard"];
}) {
  return (
    <div className="overflow-x-auto border border-white/12 bg-[#090d11] shadow-[0_2rem_5rem_rgb(0_0_0_/_30%)]">
      <div className="min-w-[42rem] lg:min-w-0">
        <div className="flex items-center justify-between border-b border-white/10 px-5 py-4">
          <div>
            <p className="text-xs font-bold uppercase tracking-[0.16em] text-white/40">
              {copy.label}
            </p>
            <h3 className="mt-2 text-xl font-semibold text-white">
              {renderWithDevoMark(copy.title)}
            </h3>
          </div>
          <div className="flex items-center gap-2 text-sm">
            <span className="border border-white/12 bg-white/[0.04] px-3 py-2 text-white/58">
              {copy.period}
            </span>
            <span className="border border-white/12 bg-white/[0.04] px-3 py-2 text-white">
              {copy.exportLabel}
            </span>
          </div>
        </div>
        {children}
      </div>
    </div>
  );
}

function UsageChart({ labels }: { labels: readonly string[] }) {
  return (
    <div className="min-w-0 border border-white/10 bg-[#0d1117] p-5">
      <div className="flex items-start justify-between gap-4">
        <div>
          <p className="text-xs font-bold uppercase tracking-[0.16em] text-white/42">
            {labels[0]}
          </p>
          <p className="mt-2 text-sm text-white/50">{labels[1]}</p>
        </div>
        <div className="text-right text-xs text-[#60A5FA]">{labels[2]}</div>
      </div>
      <div className="mt-6 flex h-48 items-end gap-2 border-b border-white/10 pl-2">
        {usageBars.map((bar, index) => (
          <div className="flex h-full flex-1 items-end" key={index}>
            <div className="flex h-full w-full flex-col justify-end gap-[2px]">
              {bar.map((height, segmentIndex) => (
                <span
                  aria-hidden="true"
                  className="block w-full"
                  key={`${index}-${segmentIndex}`}
                  style={{
                    backgroundColor: modelColors[segmentIndex],
                    height: `${height}%`,
                    opacity: segmentIndex === 0 ? 0.95 : 0.8,
                  }}
                />
              ))}
            </div>
          </div>
        ))}
      </div>
      <div className="mt-4 flex flex-wrap gap-x-4 gap-y-2 text-xs text-white/52">
        {labels.slice(3).map((label, index) => (
          <span className="inline-flex items-center gap-2" key={label}>
            <span
              aria-hidden="true"
              className="h-2.5 w-2.5"
              style={{ backgroundColor: modelColors[index] }}
            />
            {label}
          </span>
        ))}
      </div>
    </div>
  );
}

function MonitoringVisual({ copy }: { copy: EnterpriseCopy["dashboard"] }) {
  return (
    <DashboardFrame copy={copy}>
      <div className="grid grid-cols-[minmax(0,1.38fr)_minmax(15rem,0.8fr)] gap-4 p-5">
        <UsageChart labels={copy.usageLabels} />
        <div className="grid min-w-0 grid-cols-2 gap-3">
          {copy.metrics.map((metric) => (
            <div
              className="min-w-0 border border-white/10 bg-[#0d1117] p-4"
              key={metric.label}
            >
              <p className="text-[0.68rem] font-bold uppercase tracking-[0.16em] text-white/38">
                {metric.label}
              </p>
              <div className="mt-4 flex items-end justify-between gap-3">
                <p className="text-3xl font-semibold text-white">{metric.value}</p>
                <p className="text-xs text-[#60A5FA]">{metric.delta}</p>
              </div>
            </div>
          ))}
        </div>
        <div className="col-span-2 border border-white/10 bg-[#0d1117] p-5">
          <div className="flex items-center justify-between">
            <p className="text-xs font-bold uppercase tracking-[0.16em] text-white/42">
              {copy.teamTitle}
            </p>
            <MiniTrend />
          </div>
          <div className="mt-4 divide-y divide-white/8">
            {copy.teamRows.map((row) => (
              <div
                className="grid grid-cols-[1fr_5rem_5rem_5rem] items-center gap-4 py-3 text-sm"
                key={row.team}
              >
                <span className="font-medium text-white/82">{row.team}</span>
                <span className="text-white/52">{row.repos}</span>
                <span className="text-white/52">{row.score}</span>
                <span className="text-right text-[#93c5fd]">{row.trend}</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </DashboardFrame>
  );
}

function QualityVisual({ copy }: { copy: EnterpriseCopy["dashboard"] }) {
  return (
    <DashboardFrame copy={copy}>
      <div className="grid grid-cols-[minmax(17rem,0.72fr)_minmax(0,1.28fr)] gap-4 p-5">
        <div className="border border-white/10 bg-[#0d1117] p-5">
          <p className="text-xs font-bold uppercase tracking-[0.16em] text-white/42">
            {copy.qualityTitle}
          </p>
          <div className="mt-7 grid place-items-center">
            <div
              className="grid h-40 w-40 place-items-center rounded-full"
              style={{
                background:
                  "conic-gradient(#60A5FA 0 82%, rgba(255,255,255,0.08) 82% 100%)",
              }}
            >
              <div className="grid h-28 w-28 place-items-center rounded-full bg-[#0d1117]">
                <span className="text-5xl font-semibold text-white">
                  {copy.qualityScore}
                </span>
              </div>
            </div>
          </div>
          <div className="mt-7 space-y-3">
            {copy.qualityRows.map((row) => (
              <div
                className="grid grid-cols-[4rem_1fr_2rem] items-center gap-3"
                key={row.label}
              >
                <span className="text-xs text-white/46">{row.label}</span>
                <span className="h-1.5 bg-white/8">
                  <span
                    className="block h-full bg-[#60A5FA]"
                    style={{ width: `${row.value}%` }}
                  />
                </span>
                <span className="text-right text-xs text-white/62">{row.value}</span>
              </div>
            ))}
          </div>
        </div>
        <div className="border border-white/10 bg-[#0d1117] p-5">
          <div className="flex items-center justify-between">
            <p className="text-xs font-bold uppercase tracking-[0.16em] text-white/42">
              {copy.qualityTitle}
            </p>
            <MiniTrend />
          </div>
          <svg
            aria-hidden="true"
            className="mt-5 h-44 w-full text-[#60A5FA]"
            preserveAspectRatio="none"
            viewBox="0 0 520 176"
          >
            <path
              d="M0 145H520M0 102H520M0 59H520"
              stroke="rgba(255,255,255,0.08)"
              strokeWidth="1"
            />
            <path
              d="M2 140C52 134 76 130 112 119C154 106 179 83 221 88C261 92 283 116 324 102C366 87 383 43 425 48C463 53 485 72 518 39"
              fill="none"
              stroke="currentColor"
              strokeLinecap="round"
              strokeWidth="3"
            />
            <path
              d="M2 140C52 134 76 130 112 119C154 106 179 83 221 88C261 92 283 116 324 102C366 87 383 43 425 48C463 53 485 72 518 39V176H2Z"
              fill="rgba(96,165,250,0.16)"
            />
          </svg>
          <div className="mt-5 grid grid-cols-2 gap-3">
            {copy.qualityRows.map((row) => (
              <div className="border border-white/8 bg-white/[0.025] p-4" key={row.label}>
                <p className="text-xs text-white/42">{row.label}</p>
                <p className="mt-3 text-3xl font-semibold text-white">{row.value}</p>
              </div>
            ))}
          </div>
        </div>
      </div>
    </DashboardFrame>
  );
}

function SecurityVisual({ copy }: { copy: EnterpriseCopy["dashboard"] }) {
  return (
    <DashboardFrame copy={copy}>
      <div className="grid grid-cols-[minmax(0,1.2fr)_minmax(16rem,0.8fr)] gap-4 p-5">
        <div className="border border-white/10 bg-[#0d1117] p-5">
          <p className="text-xs font-bold uppercase tracking-[0.16em] text-white/42">
            {copy.securityTitle}
          </p>
          <div className="mt-5 space-y-3">
            {copy.securityRows.map((row) => (
              <div
                className="grid grid-cols-[1fr_auto] gap-4 border border-white/8 bg-white/[0.025] p-4"
                key={row.label}
              >
                <div>
                  <p className="text-sm font-semibold text-white">{row.label}</p>
                  <p className="mt-1 text-xs leading-5 text-white/42">{row.body}</p>
                </div>
                <span className="self-start border border-[#ff941f]/30 bg-[#ff941f]/12 px-2 py-1 text-xs font-bold text-[#ffbd75]">
                  {row.severity}
                </span>
              </div>
            ))}
          </div>
        </div>
        <div className="border border-white/10 bg-[#0d1117] p-5">
          <p className="text-xs font-bold uppercase tracking-[0.16em] text-white/42">
            {copy.securityTitle}
          </p>
          <div className="mt-6 space-y-4">
            {copy.securityRows.map((row, index) => (
              <div className="grid grid-cols-[2rem_1fr] gap-3" key={row.label}>
                <div className="flex h-8 w-8 items-center justify-center border border-[#60A5FA]/30 bg-[#60A5FA]/10 font-mono text-xs font-bold text-[#93c5fd]">
                  {String(index + 1).padStart(2, "0")}
                </div>
                <div>
                  <p className="text-sm font-medium text-white/82">{row.label}</p>
                  <div className="mt-2 h-1.5 bg-white/8">
                    <div
                      className="h-full bg-[#60A5FA]"
                      style={{ width: `${92 - index * 17}%` }}
                    />
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>
    </DashboardFrame>
  );
}

function EnterpriseFeatureSection({
  dashboard,
  feature,
  index,
}: {
  dashboard: EnterpriseCopy["dashboard"];
  feature: EnterpriseFeature;
  index: number;
}) {
  const visual =
    index === 0 ? (
      <MonitoringVisual copy={dashboard} />
    ) : index === 1 ? (
      <QualityVisual copy={dashboard} />
    ) : (
      <SecurityVisual copy={dashboard} />
    );

  return (
    <section className="grid gap-8 border-t border-white/10 py-12 lg:grid-cols-[minmax(18rem,0.54fr)_minmax(0,1.46fr)] lg:items-center lg:gap-14">
      <div className="grid grid-cols-[3.25rem_1fr] gap-5 lg:block">
        <div className="flex h-14 w-14 items-center justify-center border border-[#60A5FA]/30 bg-[#60A5FA]/10 font-mono text-sm font-bold text-[#93c5fd]">
          {String(index + 1).padStart(2, "0")}
        </div>
        <div className="lg:mt-8">
          <h3 className="max-w-md text-2xl font-semibold tracking-normal text-white sm:text-3xl">
            {feature.title}
          </h3>
          <p className="mt-4 max-w-md text-base leading-7 text-white/54">
            {feature.body}
          </p>
        </div>
      </div>
      {visual}
    </section>
  );
}

export function EnterpriseSection({ copy }: EnterpriseSectionProps) {
  return (
    <section className="bg-[#070a0f] px-5 py-24 sm:px-8 lg:px-10">
      <div className="mx-auto max-w-[92rem]">
        <div className="grid gap-8 pb-12 lg:grid-cols-[minmax(30rem,0.72fr)_minmax(0,1.28fr)] lg:items-end lg:gap-14">
          <div>
            <p className="text-xs font-extrabold uppercase tracking-[0.18em] text-[#60A5FA]">
              {copy.kicker}
            </p>
            <h2 className="mt-5 max-w-xl text-4xl font-semibold tracking-normal text-white sm:text-5xl">
              {copy.title}
            </h2>
          </div>
          <div>
            <p className="max-w-3xl text-lg leading-8 text-white/62">
              {renderWithDevoMark(copy.body)}
            </p>
            <div className="mt-6 border-t border-white/12 pt-5 text-sm leading-6 text-white/50">
              {copy.footer}
            </div>
          </div>
        </div>
        {copy.features.map((feature, index) => (
          <EnterpriseFeatureSection
            dashboard={copy.dashboard}
            feature={feature}
            index={index}
            key={feature.title}
          />
        ))}
      </div>
    </section>
  );
}
