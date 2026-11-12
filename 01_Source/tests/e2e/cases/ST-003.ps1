# ST-003 - the Japanese system language: the profile of the product appears, is activated, types, and the original
# input method comes back unchanged.
# The judgement of the design (Programming_IME_ST_Test_Design_v1.0.md, ST-003):
#   "PIME appears / is activated / types; the original input method and the default settings are undamaged".
# The steps are executed by the shared procedure Invoke-E2EProfileCase (lib/Cases.ps1).
function Invoke-E2ECaseST003 {
    param([Parameter(Mandatory = $true)]$Context)
    $result = New-E2ECaseResult -Id 'ST-003' -Title 'the Japanese system language activates the product and returns to the original input method'
    return (Invoke-E2EProfileCase -Context $Context -Result $result -LanguageKey 'ja')
}
