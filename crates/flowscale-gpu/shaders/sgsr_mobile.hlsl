// Clean-room 1-pass mobile spatial edge-aware upscaler (cs_5_0).
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

float Luma(float3 c)
{
    return dot(c, float3(0.2126f, 0.7152f, 0.0722f));
}

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

    float2 pos = (float2(localTid) + 0.5f) * float2(InputSize) / float2(dstSize) - 0.5f;
    int2 baseCoord = int2(floor(pos));
    float2 f = pos - float2(baseCoord);

    int2 c00 = clamp(baseCoord, int2(0, 0), int2(InputSize) - 1);
    int2 c10 = clamp(baseCoord + int2(1, 0), int2(0, 0), int2(InputSize) - 1);
    int2 c01 = clamp(baseCoord + int2(0, 1), int2(0, 0), int2(InputSize) - 1);
    int2 c11 = clamp(baseCoord + int2(1, 1), int2(0, 0), int2(InputSize) - 1);

    float4 p00 = InputTex.Load(int3(c00, 0));
    float4 p10 = InputTex.Load(int3(c10, 0));
    float4 p01 = InputTex.Load(int3(c01, 0));
    float4 p11 = InputTex.Load(int3(c11, 0));

    float l00 = Luma(p00.rgb);
    float l10 = Luma(p10.rgb);
    float l01 = Luma(p01.rgb);
    float l11 = Luma(p11.rgb);

    float edgeX = abs(l10 - l00) + abs(l11 - l01);
    float edgeY = abs(l01 - l00) + abs(l11 - l10);
    float steepX = 1.0f + saturate(edgeX * 4.0f) * 2.5f;
    float steepY = 1.0f + saturate(edgeY * 4.0f) * 2.5f;

    float2 warpedF = float2(
        saturate((f.x - 0.5f) * steepX + 0.5f),
        saturate((f.y - 0.5f) * steepY + 0.5f)
    );

    float4 top = lerp(p00, p10, warpedF.x);
    float4 bot = lerp(p01, p11, warpedF.x);
    float4 result = lerp(top, bot, warpedF.y);

    if (Sharpness > 0.001f)
    {
        float3 avg = 0.25f * (p00.rgb + p10.rgb + p01.rgb + p11.rgb);
        float amt = saturate(Sharpness * 0.09f);
        result.rgb += (result.rgb - avg) * amt;
    }

    OutputTex[tid.xy] = float4(result.rgb, result.a);
}
