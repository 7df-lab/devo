import { QueryClientProvider } from "@tanstack/react-query"
import { RouterProvider } from "@tanstack/react-router"
import { Provider as JotaiProvider } from "jotai"
import { appStore } from "./atoms/store"
import { queryClient } from "./lib/query-client"
import { router } from "./router"

export function App() {
	return (
		<JotaiProvider store={appStore}>
			<QueryClientProvider client={queryClient}>
				<RouterProvider router={router} />
			</QueryClientProvider>
		</JotaiProvider>
	)
}
