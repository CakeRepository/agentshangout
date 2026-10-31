# Creates the server in its own resource group, so deleting that group removes everything:
#   az group delete -n agenthangout-rg
# Writes the public address and the code for starting hangouts to deploy\azure.local.env.
param(
    [string]$ResourceGroup = 'agenthangout-rg',
    [string]$Location = 'northcentralus',
    [string]$Size = 'Standard_B1s',
    [string]$DnsLabel = 'agenthangout',
    [string]$User = 'azureuser',
    [string]$Key = "$HOME\.ssh\agenthangout_azure"
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

if (-not (Test-Path $Key)) {
    ssh-keygen -q -t ed25519 -N '' -C agenthangout -f $Key
}

az group create --name $ResourceGroup --location $Location --tags project=agenthangout --output none

az vm create --resource-group $ResourceGroup --name agenthangout `
    --image Ubuntu2404 --size $Size `
    --storage-sku StandardSSD_LRS --os-disk-size-gb 32 `
    --admin-username $User --ssh-key-values "$Key.pub" `
    --public-ip-sku Standard --public-ip-address-dns-name $DnsLabel `
    --nsg agenthangout-nsg --nsg-rule NONE --output none

# Web traffic from anywhere; SSH only from this machine.
$myIp = (Invoke-RestMethod https://api.ipify.org).Trim()
az network nsg rule create --resource-group $ResourceGroup --nsg-name agenthangout-nsg --name web `
    --priority 1000 --access Allow --protocol Tcp --destination-port-ranges 80 443 --output none
az network nsg rule create --resource-group $ResourceGroup --nsg-name agenthangout-nsg --name ssh-admin `
    --priority 1010 --access Allow --protocol Tcp --destination-port-ranges 22 `
    --source-address-prefixes "$myIp/32" --output none

$publicHost = az network public-ip show --resource-group $ResourceGroup --name agenthangoutPublicIP `
    --query dnsSettings.fqdn --output tsv
$createCode = [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(16)).ToLower()
Set-Content "$PSScriptRoot\azure.local.env" "PUBLIC_HOST=$publicHost`nCREATE_CODE=$createCode"

$login = "$User@$publicHost"
$ssh = @('-i', $Key, '-o', 'StrictHostKeyChecking=accept-new')
scp @ssh "$PSScriptRoot\provision.sh" "$PSScriptRoot\agenthangout.service" "$PSScriptRoot\agenthangout-backup.service" `
    "$PSScriptRoot\agenthangout-backup.timer" "$PSScriptRoot\agenthangout-backup" "${login}:"
ssh @ssh $login "sudo PUBLIC_HOST=$publicHost CREATE_CODE=$createCode BUILD_USER=$User bash provision.sh"

Write-Host "Server ready at $publicHost. Run deploy\release.ps1 to build and start the app."
