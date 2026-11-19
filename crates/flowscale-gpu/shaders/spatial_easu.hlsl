// Clean-room edge-directed Lanczos2 spatial upscaler (cs_5_0).
// Subtype: 0 = Original (16-tap), 1 = Optimized (12-tap cross).
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

float Luma(float3 rgb)
{
    return dot(rgb, float3(0.2126f, 0.7152f, 0.0722f));
}

float Lanczos2(float x)
{
    float ax = abs(x);
    if (ax < 1e-4f)
        return 1.0f;
    if (ax >= 2.0f)
        return 0.0f;
    float px = 3.14159265f * ax;
    return (sin(px) / px) * (sin(0.5f * px) / (0.5f * px));
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

    float2 uv = (float2(localTid) + 0.5f) / float2(dstSize);
    float2 pos = uv * float2(InputSize) - 0.5f;
    int2 baseCoord = int2(floor(pos));
    float2 fracPos = pos - float2(baseCoord);

    int2 c00 = clamp(baseCoord, int2(0, 0), int2(InputSize) - 1);
    int2 c10 = clamp(baseCoord + int2(1, 0), int2(0, 0), int2(InputSize) - 1);
    int2 c01 = clamp(baseCoord + int2(0, 1), int2(0, 0), int2(InputSize) - 1);
    int2 c11 = clamp(baseCoord + int2(1, 1), int2(0, 0), int2(InputSize) - 1);

    float l00 = Luma(InputTex.Load(int3(c00, 0)).rgb);
    float l10 = Luma(InputTex.Load(int3(c10, 0)).rgb);
    float l01 = Luma(InputTex.Load(int3(c01, 0)).rgb);
    float l11 = Luma(InputTex.Load(int3(c11, 0)).rgb);

    float gx = (l10 + l11) - (l00 + l01);
    float gy = (l01 + l11) - (l00 + l10);
    float gradMag = sqrt(gx * gx + gy * gy);

    float2 edgeDir = (gradMag > 1e-4f) ? float2(-gy, gx) / gradMag : float2(1.0f, 0.0f);

    float4 accum = 0.0f;
    float weightSum = 0.0f;
    float3 minC = 1e9f;
    float3 maxC = -1e9f;

    [unroll]
    for (int j = -1; j <= 2; ++j)
    {
        int sy = clamp(baseCoord.y + j, 0, int(InputSize.y) - 1);
        [unroll]
        for (int i = -1; i <= 2; ++i)
        {
            if (Subtype != 0u && (i == -1 || i == 2) && (j == -1 || j == 2))
                continue;

            int sx = clamp(baseCoord.x + i, 0, int(InputSize.x) - 1);
            float2 d = float2(float(i), float(j)) - fracPos;
            float along = dot(d, edgeDir);
            float perp = dot(d, float2(-edgeDir.y, edgeDir.x));
            float w = Lanczos2(along * 0.9f) * Lanczos2(perp * (1.0f + saturate(gradMag * 2.0f) * 0.25f));

            float4 tap = InputTex.Load(int3(sx, sy, 0));
            accum += tap * w;
            weightSum += w;

            if (i >= 0 && i <= 1 && j >= 0 && j <= 1)
            {
                minC = min(minC, tap.rgb);
                maxC = max(maxC, tap.rgb);
            }
        }
    }

    float4 outColor = accum / max(weightSum, 1e-5f);
    outColor.rgb = clamp(outColor.rgb, minC, maxC);

    if (Sharpness > 0.001f)
    {
        float3 mid = 0.5f * (minC + maxC);
        outColor.rgb = clamp(outColor.rgb + (outColor.rgb - mid) * saturate(Sharpness * 0.1f), minC - abs(minC) * 0.05f, maxC + abs(maxC) * 0.05f);
    }

    OutputTex[tid.xy] = float4(outColor.rgb, outColor.a);
}
