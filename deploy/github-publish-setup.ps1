# Run once, signed in to Azure (az login) and GitHub (gh auth login) as someone who can add apps to
# the directory and give roles on the server. Safe to run again.
#
# It lets the Publish workflow run a command on the server with no key stored in GitHub. Azure trusts
# one thing: a job from this repository in its production environment, which waits for a person to
# approve it. That is root on this one server, and nothing else in the subscription.
#
# To undo it: az ad app delete --id <AZURE_CLIENT_ID>, and the Publish workflow can't reach the server.
param(
    [string]$Repo = 'CakeRepository/agentshangout',
    [string]$ResourceGroup = 'agenthangout-rg',
    [string]$Server = 'agenthangout',
    [string]$AppName = 'agenthangout-github-publish'
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$clientId = az ad app list --display-name $AppName --query '[0].appId' --output tsv
if (-not $clientId) {
    $clientId = az ad app create --display-name $AppName --sign-in-audience AzureADMyOrg `
        --query appId --output tsv
}

$principalId = az ad sp list --filter "appId eq '$clientId'" --query '[0].id' --output tsv
if (-not $principalId) {
    $principalId = az ad sp create --id $clientId --query id --output tsv
}

$subject = "repo:${Repo}:environment:production"
$known = az ad app federated-credential list --id $clientId --query "[?subject=='$subject'].name" --output tsv
if (-not $known) {
    $credential = New-TemporaryFile
    @{
        name      = 'github-production'
        issuer    = 'https://token.actions.githubusercontent.com'
        subject   = $subject
        audiences = @('api://AzureADTokenExchange')
    } | ConvertTo-Json | Set-Content $credential
    az ad app federated-credential create --id $clientId --parameters "@$credential" --output none
    Remove-Item $credential
}

$serverId = az vm show --resource-group $ResourceGroup --name $Server --query id --output tsv
az role assignment create --assignee-object-id $principalId --assignee-principal-type ServicePrincipal `
    --role 'Virtual Machine Contributor' --scope $serverId --output none

# These name the sign-in and aren't secret. The workflow skips deploying until they're set.
gh variable set AZURE_CLIENT_ID --repo $Repo --body $clientId
gh variable set AZURE_TENANT_ID --repo $Repo --body (az account show --query tenantId --output tsv)
gh variable set AZURE_SUBSCRIPTION_ID --repo $Repo --body (az account show --query id --output tsv)

Write-Host "Publish can now reach $Server. The next merge to main will wait for an approval."
