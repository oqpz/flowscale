// Clean-room sharp-bilinear + 5-tap contrast-adaptive sharpening (cs_5_0).
cbuffer UpscaleParams : register(b0)
{
    uint2 InputSize;
    uint2 OutputSize;
    uint4 DstRect;
    float Sharpness;
    uint ModeFlags;
    uint Subtype;
    uint VrsFlag;
};

Texture2D<float4> InputTex : register(t0);
RWTexture2D<float4> OutputTex : register(u0);
SamplerState LinearClamp : register(s0);

[numthreads(8, 8, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= OutputSize.x || tid.y >= OutputSize.y)
        return;

    if (tid.x < DstRect.x || tid.y < DstRect.y ||
        tid.x >= DstRect.x + DstRect.z || tid.y >= DstRect.y + DstRect.w)
    {
        OutputTex[tid.xy] = float4(0.0f, 0.0f, 0.0f, 1.0f);
        return;
    }

    uint2 localTid = tid.xy - DstRect.xy;
    uint2 dstSize = max(DstRect.zw, uint2(1, 1));

    float2 scale = float2(dstSize) / float2(InputSize);
    float2 texel = (float2(localTid) + 0.5f) / scale;
    float2 texelFloor = floor(texel - 0.5f) + 0.5f;
    float2 f = texel - texelFloor;

    float2 prescale = max(scale, 1.0f);
    float2 sharpF = saturate((f - 0.5f) * prescale + 0.5f);
    float2 sampleCoord = (texelFloor + sharpF) / float2(InputSize);

    float4 center = InputTex.SampleLevel(LinearClamp, sampleCoord, 0);

    if (Sharpness > 0.001f)
    {
        float2 invIn = 1.0f / float2(InputSize);
        float3 n = InputTex.SampleLevel(LinearClamp, sampleCoord + float2(0, -invIn.y), 0).rgb;
        float3 s = InputTex.SampleLevel(LinearClamp, sampleCoord + float2(0,  invIn.y), 0).rgb;
        float3 w = InputTex.SampleLevel(LinearClamp, sampleCoord + float2(-invIn.x, 0), 0).rgb;
        float3 e = InputTex.SampleLevel(LinearClamp, sampleCoord + float2( invIn.x, 0), 0).rgb;

        float3 avg = 0.25f * (n + s + w + e);
        float3 detail = center.rgb - avg;
        float amt = saturate(Sharpness * 0.1f);
        center.rgb += detail * amt;
    }

    OutputTex[tid.xy] = center;
}
