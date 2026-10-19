import type { Api, Model } from "@earendil-works/pi-ai";
import type { AuthStatus } from "../../core/auth-storage.js";

export interface OnboardingSettingsReader {
	getOnboardingShown(): boolean;
}

export interface OnboardingModelRegistryReader {
	refresh(): void;
	hasConfiguredAuth(model: Model<Api>): boolean;
	getProviderAuthStatus(provider: string): AuthStatus;
}

export interface OnboardingStartupState {
	settingsManager: OnboardingSettingsReader;
	modelRegistry: OnboardingModelRegistryReader;
	model: Model<Api> | undefined;
}

export function isOnboardingModelReady(state: OnboardingStartupState): boolean {
	return state.model !== undefined && state.modelRegistry.hasConfiguredAuth(state.model);
}

export function shouldRunOnboarding(state: OnboardingStartupState): boolean {
	if (state.settingsManager.getOnboardingShown()) {
		return false;
	}
	state.modelRegistry.refresh();
	return !isOnboardingModelReady(state);
}


/** Skip the default provider login when a host deliberately hides that provider. */
export function shouldRunDefaultProviderLoginFallback(
	availableModelCount: number,
	defaultProviderId: string,
	excludedLoginProviderIds: readonly string[] = [],
): boolean {
	return availableModelCount === 0 && !excludedLoginProviderIds.includes(defaultProviderId);
}
