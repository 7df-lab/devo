// Test-only host boundaries. Real automation components, Jotai atoms, label
// helper and UI primitives remain unmodified; no server or native picker runs.
export const useParams = () => ({ automationId: "automation" })
export const useNavigate = () => () => {}
export const useProviders = () => ({ data: undefined })
export const useDevoAgents = () => ({ agents: [] })
export const getModelVariants = () => []
export const AgentSelector = () => null
export const ModelSelector = () => null
export const VariantSelector = () => null
export const pickDirectory = async () => null
export const updateAutomation = async (value: unknown) => {
 document.getElementById("saved-workspaces")!.textContent = JSON.stringify(value)
}
export const createAutomation = updateAutomation
export const deleteAutomation = async () => {}
export const runAutomationNow = async () => {}
export const archiveAutomationRun = async () => {}
export const markAutomationRunRead = async () => {}
