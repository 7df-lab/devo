export function DevoSplashBrand({ className = "" }: { className?: string }) {
	const classes = ["devo-splash-brand", className].filter(Boolean).join(" ")

	return (
		<span className={classes} role="img" aria-label="Devo">
			<span className="devo-splash-brand-mark-frame" aria-hidden="true">
				<svg
					viewBox="0 0 128 128"
					fill="none"
					xmlns="http://www.w3.org/2000/svg"
					className="devo-splash-brand-mark devo-brand-mark-attention"
				>
					<g fill="none" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeWidth="6">
						<path d="M64 17.5C78.5 25.2 86.2 40.8 82.6 56.8C67.4 57.6 54.3 49.9 48.8 36.1C51.9 26.7 57.2 20.4 64 17.5Z" />
						<path
							d="M64 17.5C78.5 25.2 86.2 40.8 82.6 56.8C67.4 57.6 54.3 49.9 48.8 36.1C51.9 26.7 57.2 20.4 64 17.5Z"
							transform="rotate(60 64 64)"
						/>
						<path
							d="M64 17.5C78.5 25.2 86.2 40.8 82.6 56.8C67.4 57.6 54.3 49.9 48.8 36.1C51.9 26.7 57.2 20.4 64 17.5Z"
							transform="rotate(120 64 64)"
						/>
						<path
							d="M64 17.5C78.5 25.2 86.2 40.8 82.6 56.8C67.4 57.6 54.3 49.9 48.8 36.1C51.9 26.7 57.2 20.4 64 17.5Z"
							transform="rotate(180 64 64)"
						/>
						<path
							d="M64 17.5C78.5 25.2 86.2 40.8 82.6 56.8C67.4 57.6 54.3 49.9 48.8 36.1C51.9 26.7 57.2 20.4 64 17.5Z"
							transform="rotate(240 64 64)"
						/>
						<path
							d="M64 17.5C78.5 25.2 86.2 40.8 82.6 56.8C67.4 57.6 54.3 49.9 48.8 36.1C51.9 26.7 57.2 20.4 64 17.5Z"
							transform="rotate(300 64 64)"
						/>
					</g>
					<circle cx="64" cy="64" r="9.5" fill="#60A5FA" />
				</svg>
			</span>
			<span aria-hidden="true" className="devo-brand-word devo-brand-word-attention">
				<span className="devo-brand-word-track">
					<span>DEVO</span>
					<span>devo</span>
					<span>Devo</span>
				</span>
			</span>
		</span>
	)
}
